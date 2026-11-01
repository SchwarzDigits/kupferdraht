# Open — Questions, Backlog & Non-Goals

Everything still on the table. Two kinds of entry: **open questions /
unknowns** (the numbered §N findings log below) and **backlog** (decided work
not yet shipped — folded in 2026-06-30 from the former `docs/v1-*-audit.md`).
Decided scope cuts are not open: they live in `docs/non-goals.md` (moved there
2026-11-01). When an item resolves it leaves here — as a permanent Wire/MLS
behaviour fact → `docs/observations.md`, as a shipped change → `CHANGELOG.md`,
as a decision not to do something → `docs/non-goals.md`. Step-sequenced
delivery lives in `PLAN.md`.

## 60) Step 29 decomposition leaves three client/ files over the 200-LoC cap — finer split deferred to a later step (2026-06-16)

Step 29 split the 1741-code-line `client/runtime.rs` monolith into ten
cohesive modules (`mod`, `session`, `profile`, `http_calls`, `sync`,
`rooms`, `membership`, `timeline_read`, `mentions`, `typing`). Three of
the resulting files still exceed the CLAUDE.md 200-code-line cap while
hosting more than one responsibility:

- `client/membership.rs` (426): invite/kick · join/resync · member-list
  sync · commit-bundle/key-package helpers
- `client/rooms.rs` (361): list accessors · create/DM · metadata ·
  join-links
- `client/sync.rs` (237): sync loop + handlers · room-cache persistence

The finer split (the plan's conditional "29g") is **deferred to a
future, separate step** — per the Step 29 decision it is explicitly NOT
part of Step 29, whose scope was eliminating the `runtime.rs` monolith.
Proposed boundaries for that later step: `sync` → `sync` + `sync_cache`;
`rooms` → `rooms` + `rooms_create` + `rooms_links`; `membership` →
`membership` + `membership_join` + `membership_members` (each lands well
under 200).

Pre-existing over-cap files outside Step 29's scope: `login_pipeline.rs`
(391), `builder.rs` (236), `resume.rs` (221).

> **Re-count 2026-07-16** (docs catch-up audit; raw `wc -l` totals
> incl. doc-comments — and for `login_pipeline.rs` / `builder.rs` /
> `resume.rs` incl. their `#[cfg(test)]` modules, so subtract tests
> before comparing against the 200-code-line cap): the window's
> SSO + wasm work grew every file on this list by 60–100% —
> `membership.rs` 709, `rooms.rs` 760, `sync.rs` 456,
> `login_pipeline.rs` 809, `builder.rs` 707, `resume.rs` 488. The split
> boundaries proposed above predate that growth — re-derive them when
> the step is picked up.

---

## 61) Residuals from the WASM/ADR-0005 divergence finding (2026-07-16; narrowed 2026-09-10)

One residual of the finding (the rest went into ADR-0015 and Step 82):

- **SSO option-β decision** (Step 17) is ADR-shaped but lives only
  in the archived plan doc (`docs/plan/archive/17-saml-sso.md`) —
  `new-adr` candidate, undecided.

## Backlog — decided work, not yet shipped

> Folded in 2026-06-30 from the former `docs/v1-*-audit.md` trackers. Gaps,
> improvements, and defects that are decided but unbuilt — none blocking, all
> quality. Step-sequenced delivery still goes through `PLAN.md`; an item
> graduates into a numbered Step when picked up.

### Caller ergonomics

Login/resume/persistence forces multi-step ceremony; accessors return `Option`
where identity is always present; some messages / CLI verbs could be friendlier.
Additive unless noted.

*Convenience APIs (additive; the explicit steps stay):*
- **erg-A.1** `ClientBuilder::login_and_unlock(email, pw, 2fa?)` → `Arc<Client>` in one call.
- **erg-A.3** `ClientBuilder::resume_and_sync()` — chain open-store + resume + unlock + initial sync.
- **erg-A.4** `ClientBuilder::with_persistence(path)` — open `path/kupferdraht.db` + set the storage path in one call.

*Infallible accessors (add alongside the `Option` ones):*
- **erg-B.1** `authenticated_user_id() -> &UserId` (today only `user_id() -> Option<&UserId>`).
- **erg-B.2** `device_id() -> &str` (today `Option<&str>`).
- **erg-B.3** borrowing / `Result` access-token getter (today `access_token() -> Option<String>` allocates).
- **erg-B.4** `qualified_user_id() -> &str` direct (today reached via `user_id()`).
- **erg-B.5** `server_config() -> &ServerConfig` (one accessor vs. separate `homeserver_url` / `websocket_url` / `server_title`).

*Error messages:*
- **erg-C.1** `pipeline.rs` "{op}: {room_id} not in cache" — add an actionable hint ("run sync / load_rooms_from_store").
- **erg-C.2** `pipeline.rs` "has no group_id — run sync_metadata first" is misleading (the real fix is to join); mirror the typed `KupferdrahtError::MlsGroupMissing` text.
- **erg-C.3** `login_pipeline.rs` "no devices row for {user}/{device}" → "device was unregistered — re-login".
- **erg-C.4** `sqlite/mod.rs` "storage column … not valid UTF-8" → add a corruption / recovery hint.
- **erg-C.5** ~244 `"{op}: {e}"` sites bubble raw errors with no layered context — add a hand-rolled error-context trait (no `anyhow`), applied at layer boundaries.
- **erg-C.6** `looks_like_uuid` shape-check in `resolve_room_for_message` with a distinct "doesn't look like a message id" message.

*CLI UX:*
- **erg-D.1** `send-file --mime` optional, auto-derive from extension (`mime_guess`); explicit flag overrides.
- **erg-D.2** `download-asset --message` partial-UUID prefix match (+ ambiguity error).
- **erg-D.3** `rejoin-all` → `rooms-rejoin-all` (group with the `rooms-*` verbs).
- **erg-D.4** Sync-command naming — the `sync` / `sync-metadata` / `sync-stream` trio is confusing; pick a consistent scheme.
- **erg-D.5** Access presets `invite-only` / `public` (only `room-access-allow-guests` exists).
- **erg-D.6** `send-file -f/--file` short alias for `--path`.
- **erg-D.7** Positional `send <room> <text>` shorthand.
- **erg-D.8** `status --verbose` (room count, store path, last-sync, config / property counts, `file_sharing`, `receipt_mode`).
- **erg-D.9** `cache list | size | clear` commands.
- **erg-D.10** Global `--log <level>` flag (the tracing subscriber init shipped; the flag did not).
- **erg-D.11** `doctor` health-check (session valid? device registered? MLS state present? key-package pool depth?).
- **erg-D.12** *(deferred)* group per-message ops under a `msg` subcommand.

*Won't-fix:* per-endpoint-group HTTP wrappers vs. a single `http()` accessor — the 9 `http_*` wrappers are fine; no change.

### Known gaps (capability / lifecycle / test coverage)

*Protocol / capability:*
- **gap-B.1** MLS ciphersuite negotiation — P-256 (`0x0002`) hardcoded; `mls.config.defaultCipherSuite` / `allowedCipherSuites` persisted but never consumed. Needs a correctly-numbered ciphersuite ADR (see def-D.1).
- **gap-B.2** Typed config getters limited to two (`file_sharing()` + `receipt_mode()`); add others (mlsE2EId, legalhold, appLock, mlsMigration, …) as a consumer needs them.

*Library / lifecycle:*
- **gap-B.4** Asset cache: no LRU eviction / size cap — grows unbounded.
- **gap-B.6** `fetch_server_configs` inline on the login path — errors bounded (warn-and-continue), latency not; spawn as a post-rebind task.
- **gap-B.8** Single-ciphersuite key-package pool — tied to gap-B.1.
- **gap-B.10** DB-key rotation not implemented — the per-DB key is load-or-create only (SQLite3MC offers `PRAGMA rekey`; unused).
- **gap-B.26** `ClientBuilder::login()` resolves the keystore (and writes the cookie label into it) BEFORE `HttpClient::new` validates the URL scheme — on a Keychain-less macOS sandbox the `http_hardening` rejection test blocks on the Keychain for ~30 min instead of failing fast (observations §76). Reorder: validate the URL before any keystore I/O; `KUPFERDRAHT_DEV_KEYSTORE` is the workaround meanwhile.
- **gap-B.27** mls.db migrations are not atomic — `openmls_sqlite_storage` 0.3.0 runs refinery non-grouped (`storage_provider.rs:62-65`, not configurable from outside), so a crash between a migration's SQL commit and its history-row commit re-applies it on the next open; V2+ of its migrations are re-apply-fatal. Step 87 closed the same window for kupferdraht.db (`set_grouped(true)`); mls.db waits for the upstream fix (issue draft: `docs/plan/87-atomic-migrations.md`) — or, since Step 127, for the SchwarzDigits fork of `openmls_sqlite_storage` (§82) to carry `set_grouped(true)` ahead of upstream. Blast radius is smaller than kupferdraht.db's: a bricked mls.db can be discarded and the groups rejoined by external commit; the account store cannot.
- **gap-B.35** The wasm `Client.rooms()` / `messages()` read the in-memory room cache, which is **cold after `resume()`** until the first `syncOnce` (or `load_rooms_from_store`): `rooms()` returns `[]` and `messages(roomId)` fails "unknown room" for a room the store holds (seen live 2026-09-25 by the promoted leader tab). The doc comment says "read from the store" — native has `room_or_load` (Step 14g) for exactly this; the wasm binding should hydrate on a cold miss (`room_or_load`, or `load_rooms_from_store` on resume) and the doc should say what it reads. Not a Step-94 change (surface semantics).
- **gap-B.30** `kupferdraht-core`'s unit-test harness is native-only: `#[cfg(test)]` modules pull `httpmock` / `tempfile` without a target gate, so `cargo test --target wasm32-unknown-unknown` (and `wasm-pack test`) cannot build the lib tests — only the wasm integration tests run (`scripts/wasm-test.sh`). Gating those modules (`cfg(all(test, not(target_arch = "wasm32")))`, the sister project's pattern) would let the pure-Rust unit tests run in the browser too; not needed by any step yet.
- **gap-B.32** Self-room establishment (Step 86) Adds every key package Wire hands out for the user's *other* registered devices (`claim … skip_own`, the `create_room` rule) — dead devices of unreadable homes included, so the self group's roster carries leaves nobody will ever operate until Wire's device-deletion Remove proposals clear them (observations §69/§70). Harmless for confidentiality (those devices are the user's own), but the roster is not "live devices"; a stale-snapshot establish race (another device commits first) is handled by discard + retry, not by a pre-flight `mls-self` fetch. `JoinSummary` deliberately gained no `established` counter (minimal API) — an established self room counts under `joined`.
- **gap-B.33** `ConvType 3` is mapped to `Channel` and `group_conv_type` is not read (`rooms/room.rs`, `conversation_type` mapping). Wire treats type 3 as a pending-connection 1:1 and signals channels through `group_conv_type: "channel"` (side finding of the 2026-10 provenance audit, `docs/wire/provenance-audit-2026-10.md`). Verify against a live v14 backend, then fix the mapping; consumers that key on `RoomType::Channel` need the note.

*Test coverage:*
- **gap-B.11** Asset send/receive happy-path not unit-testable (httpmock can't stand in for openmls; gated on live-smoke).
- **gap-B.12** AES-256-CBC known-answer vectors absent; generate them with an independent tool (`openssl enc`), never from a Wire repository (ADR-0011 revision 1.2).
- **gap-B.13** Live `/feature-configs` fixture not captured (tests use a synthesised one).
- **gap-B.14** Wire-Web→Kupferdraht inbound asset never live-validated.
- **gap-B.15** `WIRE_RECEIPT_MODE` off-path smoke not run against a live backend.
- **gap-B.16** 4xx httpmock coverage not symmetric (assets / feature-configs / properties / token-refresh covered, not all endpoints).
- **gap-B.7 residual** — the low-pool live drain of the key-package top-up (Step 63) is untested until a claim source exists (observations §66).
- **gap-B.25 residual** — the removal flush (`sync/removal_flush.rs`, Step 74; observations §69/§70) keeps its pending set in memory: a restart loses it; queued proposals persist in openmls storage and commit on the room's next activity (see also §73).
- **gap-B.23** Pull crash-window double-dispatch (review S8
  residual, 2026-08-14): the Step-64 dedup catches only a notification
  whose id equals the page cursor (the push-symmetric case). The
  original window — crash after dispatch, before `save_sync_state`,
  where the unsaved notification carries a *different* id — is not
  closable by cursor equality at all; it needs idempotent dispatch or
  a processed-ids journal. Text is softened by UNIQUE + the S6 guard;
  replayed MLS ciphertexts die at replay protection (spurious
  recovery now excluded by the Step-64 narrowing). Own design pass
  when it matters.
- **gap-B.22** *(upstream openmls)* No staged external join: the
  `ExternalCommitBuilder` chain persists **and merges** the group
  atomically in `finalize` — there is no way to build the external
  commit, upload it, and merge only on server acceptance (the
  add/remove path's `upload_and_merge_commit_bundle` pattern).
  Step 66 therefore ships discard-on-upload-failure instead of true
  staging (decided 2026-08-13, Volker). Worth an upstream issue /
  feature request against openmls: a `StagedExternalJoin` that
  defers the storage merge. Revisit Step 66's residual crash window
  if upstream ever ships it. Checked against openmls 0.9.0
  (2026-08-25): still not shipped — the Step-66 discard workaround
  stays; the upstream issue remains worth filing.
*Schema / process hygiene:*
- **gap-B.17** Hand-written migrations only (V1–V30 by hand).

*Defects (wrong references, not shortcuts):*
- **def-D.2** Preview ADR unwritten (NG-2, consumer-demand-triggered); the asset-deletion half is written in **Step 84**.

### Code quality / maintainability

Correct code that is hard to read / navigate. The baseline safety rules (no
`unsafe` / `unwrap` / `println!` / `macro_rules!`; clean build / clippy /
`cargo deny`) are re-checked each gate run — not tracked here.

- **cq-A.1** Files over the 200-code-line cap. Remaining offenders **as of the 2026-06-16 sweep — re-count before acting** (Steps 34–42 have since moved code): the three Step-29 residues `client/membership.rs` · `client/rooms.rs` · `client/sync.rs` (see §60), plus `rooms/room.rs`, `client/login_pipeline.rs`, `sync/service.rs`, and pre-existing `wire/http/conversations.rs`, `ffi/timeline.rs`, `wire/dto.rs`, `wire/proto.rs`, `storage/records.rs`, `manager/mod.rs`.
- **cq-A.2** `process_inbound_message` — forbidden `process` prefix. `MlsClient::process_inbound_message` (`encryption/messages.rs`) decodes / absorbs an inbound MLS message → name it for the effect (`decode_inbound_message` / `absorb_inbound_message`).
- **cq-A.3** Three `#[allow(clippy::struct_field_names)]` over renamable fields: `rooms/room.rs` (`room_type`→`kind`), `client/mod.rs` (`mls_client:`→`mls`), `ffi/sync.rs` (FFI `Message` `message_*` fields).
- **cq-A.4** Redundant `_with_storage` suffix on `MlsClient::new_fresh_with_storage` / `resume_with_storage` — pure rename (the in-memory `new_fresh` wraps the `_with_storage` variant; not dead).
- **cq-B.1** Magic byte arrays in fixtures (`vec![0xNN; N]`, no shared names) — introduce named `const`s.
- **cq-B.2** No property-based tests (`proptest` / `quickcheck`) — crypto + protobuf round-trips are candidates.
- **cq-B.3** No benchmarks (`criterion`) — add when perf becomes a question.

### Blocked on the Wire API (no endpoint yet)

- **Step 12 — Key backup (mnemonic):** blocked — Wire v14 exposes no backup endpoint (`docs/observations.md` §62).
- **Step 62 — `/v14/events` push (dormant):** wire-server
  hard-codes `consumableNotifications: false` since chart 5.30 (PR
  #5116 hard-codes the flag, no public timeline — observations
  §68). The transport is merged + archived as opt-in, self-guarded by
  `server_supports_push_events()`. **Re-check** on every wire-server
  upgrade of the own instance: `GET /v14/feature-configs →
  consumableNotifications.status`; when `enabled`, run the archived
  `smoke-step62` flow (its 7 stages are documented in
  `docs/plan/archive/62-events-websocket.md`).

### Proposed steps (need Volker's OK before a PLAN.md row)

- **`LastRead` cross-device read position over the self room** —
  proposed 2026-09-24 (Step 86 retro): send + fold Wire's `LastRead`
  in the self conversation so `mark_as_read` on one device moves the
  read marker on the user's others (§64 "Conversation daily life").
- **Draft sync over the self room** — proposed 2026-09-24: per-room
  drafts as self-room messages (client-local drafts first).
- **Personal task list → self room, incl. the Step-89 `DeleteForMe`
  path** — proposed 2026-09-24: move ADR-0014's "own list" from the
  1-member workaround group into the self conversation and route the
  receiver-side ephemeral expiry's `DeleteForMe` through it
  (`docs/plan/archive/89-ephemeral-messages.md`).
- **Idempotency keys for wasm mutations (leader-change retry)** —
  proposed 2026-09-25 (Step 94 retro, ADR-0026 L3-C): today a mutation in
  flight when the leader tab changes fails `LeaderChanged` and is never
  retried by `kupferdraht-tabs`, because the module cannot know whether the
  old leader's send reached the backend. A client-chosen idempotency key
  per mutating call (`sendText`, `sendReply`, `sendFile`, `createRoom`,
  …) stored with the outcome in the leader's store would let the proxy
  retry safely on the new leader (the same key returns the stored
  handle or re-runs). Small on the wasm surface (one optional argument
  or an options object per mutation), a new table + a dedup lookup in
  the core; the Step-79 resend path (not on wasm yet) is the honest
  interim.
- **kupferdraht-tui Ctrl-R onto the SDK resend API** — proposed 2026-09-24
  (Step 79 retro, consumer repo): replace the TUI's `PendingKind`
  re-drive (Text / Reply / File) with `Room::failed_sends` +
  `Timeline::resend_failed` on the first successful sync iteration
  after a reconnect, plus a per-row `Timeline::resend` for Ctrl-R;
  `PendingKind::Edit` stays TUI-side (a failed edit is not a row).

## 64) Chat-system basics — the post-Step-60 gap map (2026-07-19; re-cut 2026-11-01)

Roadmap discussion after tasks shipped: what a daily-driver messenger
still needs. Backlog candidates, grouped; PLAN.md steps get generated
from here with the user's OK. Items already tracked elsewhere are
referenced, not duplicated. Re-cut 2026-11-01: contact discovery and
direct contact initiation are non-goals by Kupferdraht's purpose
(`docs/non-goals.md` NG-18); push registration, new-device history and
presence are recorded there too (NG-19 to NG-21); link previews
contradicted NG-4 and are gone.

### Conversation daily life

- **Mute / archive** — Wire carries `muted_status` / `archived` per
  conversation; small step, high everyday value.
- **Cross-device read position** — Wire syncs "read up to" via
  `LastRead` through the self-conversation; the channel exists since
  Step 86 (2026-09-24) — next rail (Proposed steps).
- **Drafts** — per-room draft persistence (client-local first;
  Wire-style sync rides the self-conversation too).
- **Local search** — under E2EE only the client can search;
  SQLCipher + FTS5 native, own path needed for IndexedDB. At a few
  thousand messages this stops being a luxury.

### Device trust

- **Device verification UX** — MLS gives cryptographic identity; since
  Step 95d a room's devices and their MLS thumbprints are readable
  (`Room::devices`, `subscribe_device_changes`), but there is no
  verification / trust state yet.

### Group governance (second tier)

- **Conversation roles** — *narrowed by Step 72 (2026-08-21):*
  roles ARE decoded, persisted (`room_members.role`, V26), merged
  through `sync_members`, and honored where authorization exists
  (the ADR-0018 case gate, `[admin]` in `room-members`). Still
  open: invite/kick are role-blind — deliberately so far, since
  Wire itself lets any member invite (observations §72); gating
  them would be a Kupferdraht-side policy decision, not parity.
- **Room avatars** — absent.

### Priority thesis

1. **The self-room follow-ups** (personal task list, `LastRead`
   cross-device read sync, draft sync) — under Proposed steps.
2. **Mute/archive** — daily comfort.
3. **Local search** as a filler between steps.

## 66) External code review triaged — 18 findings verified, dispositions assigned (2026-08-13)

Of the 18 findings one is still open: **S11** (task-cache
monotonicity) is a design question — ADR-0014's last-writer-wins means
*event order*; a wall-clock guard would change semantics; revisit if
cursor-reset replays become a real flow. (The dispositions of the
others: `docs/plan/archive/`, `CHANGELOG.md`.)


## 68) `invite_members` freshness is a contract, not enforced (2026-08-26)

The ADR-0020 invite hook hands the invitees this client's *current*
fold of case/marker/tasks — state the inviter has not drained is
permanently missing for the joiner (MLS has no history). Today that is
a documented contract on `invite_members` (the CLI drains first;
push-loop consumers are always fresh). Open decision: should
`invite_members` drain notifications itself (core-side enforcement —
costs a sync round-trip on every invite, duplicates the consumer's
loop) or stay a documented contract? Revisit if a second consumer
trips over it.

## 69) Kalium-audit residuals — checked, accepted, not commissioned (2026-09-10)

The Kalium-JVM production-readiness audit was cross-checked against
Kupferdraht (Steps 77–80 commissioned from the real findings; the
blocker class does not transfer — one Rust core, SQLCipher
everywhere, calling out of scope). Two minors were verified and
deliberately left as residuals rather than steps:

- **DB files carry umask default modes**, not 0600 (no
  `OpenOptions`/`set_permissions` anywhere in the storage layer).
  Accepted because the content is SQLCipher ciphertext and the keys
  live in the OS keystore — mode 0600 would be defense-in-depth
  only. Bundle it into whichever step next touches the open paths.
- **Windows `FileKeyStore` writes without ACLs** (the
  `cfg(not(unix))` arm is plain `fs::write`). Accepted: the file
  keystore is a dev-only escape hatch (Step 77 makes it loud and
  atomic; there is no Windows CI to regress against anyway — the
  CI-matrix gap itself is pinned in the 20e support-tier ADR
  precondition).

*2026-10-08, decided (Volker):* the findings stay named in the public
repository as written here and in Steps 77–80; they are documented in
Kalium's own pull requests and were the checklist, not the source, of
Kupferdraht's hardening.

## 70) Step 95c residuals — cache-lock `expect`s, payload length caps, function-length debt (2026-09-26)

Left open by Step 95c (`docs/plan/archive/95-inbound-message-integrity.md`),
the first two deliberately:

- **About 26 `.expect("… poisoned")` on in-memory cache locks** — the
  rooms cache, event-handler and listener registries, typing state
  (`client/{sync,rooms,membership,timeline_read,typing}.rs`,
  `sync/service.rs`). They fire only after another panic while the
  lock was held — no foreign data reaches them — but they turn one
  defect into a cascade. Recovering them the way `lock_mls` does is
  safe for data that is replaced wholesale; bundle it into the next
  step that touches these modules.
- **No length caps on Kupferdraht's own payloads** (`TaskPayload`,
  `CasePayload`, `extensions`) — a peer can send a very large title or
  map; it is stored as-is. Needs a cap per field — a protocol decision
  (what a receiver drops, what a sender refuses), so ADR material.
- **Function-length debt the step touched.** Functions already over
  the 40-line budget before 95c grew by a few lines each — `run_push`
  (61), `ClientBuilder::login` (52), `create_room` (50), `delete_room`
  (46), `absorb_welcome` (44), `dispatch_inbound_ephemeral` (44),
  `upsert_task` (49) — and `start_sync` crossed it (41). Other pre-95
  overlong functions in the same files (`dispatch_generic_message` 66,
  `rebind_access_token` 68, `create_dm` 62) are older debt. Split them
  when the module is next touched.

## 71) Production-readiness review — open findings (review 2026-09-25 at `e85b438`, re-checked 2026-09-26 at `5f7eef4`)

What the production-readiness review commissioned by Markus (four
parallel code audits — MLS/crypto, local secrets, network/sync,
state/concurrency) left open after Steps 95–106 closed its blockers 1–3
and 5–9 and findings H1–H4, H6, H7 part 1 and H8 (the record:
`docs/plan/archive/`, `CHANGELOG.md`). Each item re-verified at
`5f7eef4` and again on 2026-11-01:

*Data loss (blockers):*
- **B4 — Linux desktop loses the DB keys on reboot.** `keyring` 3.6.3
  with `linux-native` pulls only `linux-keyutils` (kernel keyring, not
  persistent — also lost when the kernel's persistent keyring expires
  after a few days logged out). Together with B5 a reboot makes the
  account unusable on that machine. **Deferred for now, not
  permanently** (Markus, 2026-09-27): taken up after the
  platform-independent blockers. Until then the docs say so
  (`Cargo.toml`, `keystore/`, `docs/library.md`, THREAT_MODEL). Fix
  sketch: the crate's own combination feature
  `linux-native-sync-persistent` (= `linux-native` + `sync-secret-service`,
  the sync libdbus backend — the async one can block inside tokio) plus
  `crypto-rust`; build/CI need `libdbus-1-dev` or the `vendored` feature
  (AFL-2.1 into `deny.toml`). Open decision (ADR):
  headless Linux without a Secret Service — unsupported, a passphrase
  keystore, or systemd-creds / TPM.
- **iOS notification extension (NSE)** — not planned; noted 2026-09-28
  while designing ADR-0031. A second process started by a push, with a
  hard time and memory budget and killed without warning, needs more
  than ADR-0031: a receive-only drain mode (no own commits, no healing,
  stop before what needs the app), a time budget, the database keys
  readable while the device is locked (shared keychain access group,
  `kSecAttrAccessibleAfterFirstUnlock`, ADR-0029), and care with iOS
  terminating a suspended process that holds a file lock in a shared
  container (`0xdead10cc`) — `kupferdraht.lock` is held across `POST
  /access`. B9 becomes likely with it.
- **Residuals of B7 and B9.** The wasm `send_bytes` path
  (`timeline/asset_send.rs`) emits no `Append` for its own row — only the
  settling `UpdateAt` (found in 97a). Custom `Store` implementations stay
  non-atomic until they override the combined writes of Step 105.

*High (before wide use):*
- **Residual of H4** — whether the backend itself refuses messages for a
  future epoch was not verified (no server source; a live probe was not
  made).
- **H5** — blocking work in async code: SQLite, fsync, crypto and
  generating 100 key packages run without `spawn_blocking`; UniFFI polls
  on the caller's thread (ANR risk on Android — derived from UniFFI
  internals, not measured).
- **Residuals of H6** — UniFFI generates the Kotlin `toString` / Swift
  `description` of FFI records itself: `AssetMeta.assetToken` still
  prints there (a consumer logging the record leaks it; N3 of the threat
  model puts that on the consumer); and a sentence "Kupferdraht writes no
  secrets into its own logs" for `docs/THREAT_MODEL.md` is proposed for
  the owner's review.

*Medium / low:*
- **M1** — WebSocket: no connect timeout; pongs are ignored (liveness
  rests on a failed ping send).
- **M2** — redirects: reqwest's default policy follows https→http, and
  on 307/308 re-sends the login body with the password to another host.
- **M3** — `ws://` is accepted without `permit_http`.
- **M4** — Windows: `is_path_safe_id` passes `:`, reserved names
  (`CON`, `NUL`) and a trailing dot or space.
- **M5** — `global.db` has no schema version; `PRAGMA secure_delete` is
  set nowhere.

*Zeroization (not a blocker; the claim is the problem):* keys and
credentials in their holders are `Zeroizing`, but "compiler-enforced"
(README, CLAUDE.md, THREAT_MODEL G4/G6) overstates it — tokens leave as
plain `String` copies (`current_access_token()`, public
`access_token()`), the login body copies the password into a
`serde_json::Value`, `SessionRecord` holds plain tokens, and message
plaintext on the send and receive paths is never zeroized (only the
resend payload is). Either narrow the claim to "best-effort
zeroization of keys and credentials" or add a non-`Clone` secret type
with redacted `Debug` end to end.

*Threat model vs. code:* G4 "forward secrecy in local storage" needs
`secure_delete`; the auth modes `PasswordProtected` / `PasswordOnly` /
`DeviceBound` do not exist in code; "reproducible builds" do not exist;
with Basic credentials the backend is the key directory and can inject
a device — "does not trust the server with content" must be stated as
an assumption (95d makes the device visible, it does not prevent it).

*Operations and process (re-checked 2026-11-01):* no external audit;
no fuzzing, no property tests, no automated interop tests against Wire;
CI runs on Ubuntu only, without the Kotlin / Swift binding tests (their
test pins have not compiled since Step 39 — observations §93; Step 20e's
macOS job is the structural answer); `v0.1.0` is tagged (2026-11-01)
without an API-stability promise (Step 20e); licensing for proprietary
consumers (GPL-3.0-only; contributions come in under the Schwarz Digits
CLA) and the Wire ToU question are business decisions.

## 72) Browser server mode — residuals of Step 111b (2026-09-30)

What Step 111b (ADR-0035, the account databases on a storage server) left
open, none of it blocking the step:

- **A long pending login.** Until `registerDevice` returns, the storage
  token is renewed with the login's Wire token, which expires after
  15 minutes; a login left pending longer retries the renewal every minute
  until the client exists. The first renewal is due after half the
  storage token's lifetime (30 minutes), so this only matters for a login
  left open that long.
- **A purge needs a live client in the browser (Step 114).**
  `logout({ purgeLocal })` runs on a `Client`: an account whose session
  expired is logged in again (its keys are here), then logged out with the
  purge. Natively Step 78 added `Kupferdraht::forget_account(account_id)`
  for exactly this; the browser gets no `forgetAccount` yet — adding it
  means a tabs-proxy mirror (Step 94's parity) and a browser test, so it
  waits for an embedder that needs it.

## 73) A failed removal flush is lost with the client (2026-10-01)

Found by the Step-117 smoke (its first logged warning): in the browser
smoke account's DM, `sendText` failed with `GroupStateError(PendingProposal)`
on every try. A backend Remove proposal sits in the room's MLS state; the
Step-74 removal flush (`sync/removal_flush.rs`) commits such proposals at
the end of a sync cycle and keeps a failed room queued for the next cycle —
but the queue lives in the `Client`'s memory. The first flush failed
(`removal flush failed; retrying next cycle`, the error not captured), the
browser was closed, and the next client started with an empty queue: the
stored proposal now blocks every send in the room until another Remove
arrives. Native clients have the same queue. Directions (not decided):
rebuild the queue at startup from the groups' stored proposals, or let the
send path commit pending proposals before it creates the message. To
diagnose first: why the flush failed.


## 74) Guest links of another operator (2026-10-05)

A guest link carries only `key` and `code`
(`https://account.<operator>/conversation-join/?key=…&code=…`, measured on
the test stage); its host is the operator's account page, not the
conversation's domain. Every code endpoint of the v14 client API is
unqualified (`POST`/`GET /conversations/join`, `code-check`,
`/conversations/{cnv}/code`): a redeem asks the user's own backend, which
knows only its own codes. So Kupferdraht redeems links of its own operator only
(Step 122 finds the room by the conversation id the link names); a link of
another operator fails with `404 no-conversation-code`. Open (Markus): is
there a federated way to join through another operator's link? Measure
with an account on a backend federated with the test stage before
deciding anything.

## 77) The second receipt interop vector is missing from Kupferdraht (2026-10-07, narrowed 2026-10-08)

The PIN service's `interop.json` carries two receipt vectors; the second
one's account domain is the dev instance's hostname, inside the signed
message. A domain swap in Kupferdraht alone would turn the interop check
into a self-test, so `recovery/receipt.rs` now carries only the first
vector (`wire.example`), which exercises the same derivations with a
different seed and a shorter domain. Follow-up: natrium-pin-service
regenerates the second vector with a neutral domain; Kupferdraht adds it
back in the same step.

## 79) Compliance workflow — two upstream issues reported to the OSS Committee (2026-10-08)

Found by Kupferdraht's first full runs of `SchwarzDigits/oss-compliance`
`full-check.yml@v1`, both outside this repository. Reported 2026-10-09 to
the maintainer of the central workflow, together with the request to
classify the five permissive licenses behind the evaluator's hints
(Unicode-3.0, CDLA-Permissive-2.0, Apache-2.0 WITH LLVM-exception,
BSD-1-Clause, BSL-1.0; see §78). Open until the central repository
changes; what Kupferdraht does then is noted per item.

- **Scanner.** The workflow passes `--scanners Askalono`; the `ort:latest`
  image it runs is ORT 96, which no longer ships that scanner (offered:
  DOS, FossId, Licensee, Nomossa, Provenant, ScanCode, SCANOSS). Every
  repository on the current image fails at the scan step. Kupferdraht
  sets the workflow input `scanner: disabled` and relies on declared
  licenses, which `cargo deny check` verifies on every commit; flip it
  back to `auto` once the central workflow names a scanner ORT 96 has.
- **Gradle.** The Gradle Inspector resolves no plugin inside the
  container (`Plugin … was not found in any of the following sources:
  Gradle Central Plugin Repository`); natrium's runs show the same
  failure on all three of its Gradle files, tolerated because the
  analyzer's exit code 2 does not fail the job. Kupferdraht excludes its
  Gradle builds from the analyzer and lists their Maven dependencies by
  hand in THIRD-PARTY-NOTICES; the JDK-17 preparation script stays for the
  day the inspector works.

Contact: opensource@digits.schwarz. Not pursued locally: silencing the
hints through `resolutions` in `.ort.yml` — technically possible, but the
program's onboarding guide asks projects not to override central policy
without the Committee, and the hints are the signal it uses to grow the
classification. Kupferdraht's own license policy for the dependency
graph remains `deny.toml`: it allows Unicode-3.0 and CDLA-Permissive-2.0
by name, accepts the other three only as alternatives of `OR`
expressions whose Apache-2.0 or MIT branch is on its list (ORT, unlike
cargo-deny, reports every alternative), and reads `stringmetrics`'s
license file where ORT reports NOASSERTION.

## 81) crypto-bigint 0.5: timing advisory without an upgrade path (2026-10-30)

RUSTSEC-2026-0387 (2026-10-23): `crypto-bigint` 0.5 can branch on secret
data in subtraction, zero counting, division, Montgomery multiplication
and limb equality, depending on compiler and CPU. Patched in 0.7.6, and
0.7 is taken only by the RustCrypto 0.14 line (`elliptic-curve` 0.14,
`p256` 0.14, `ecdsa` 0.17). Kupferdraht reaches `crypto-bigint` 0.5
through `p256` 0.13 from three places: `openmls_rust_crypto` 0.6 (HPKE
over `hpke-rs-rust-crypto` 0.7, the key-package and Welcome path of
ciphersuite MLS_128_DHKEMP256_AES128GCM_SHA256_P256), `openmls_basic_credential`
0.6 (the ECDSA P-256 signatures on MLS messages) and `voprf` 0.5 (the
PIN service's OPRF, RFC 9497 with P-256). None of the three has a release
on the 0.14 line; `hpke-rs-rust-crypto` 0.8.0 (2026-10-07) has moved.

Accepted 2026-10-30 as a `deny.toml` ignore with the reason recorded
there, because the crates cannot move independently and the alternative
is no build until openmls follows. The advisory concerns side channels
an attacker must be able to observe on the device running Kupferdraht;
nothing in the protocol or the stored data changes.

Upstream state on 2026-10-30: openmls `main` migrated its crypto crates
to `p256` 0.14 and `hpke-rs-rust-crypto` 0.8 on 2026-10-02 (PR #2249),
unreleased — the next `openmls_rust_crypto` / `openmls_basic_credential`
release carries it. `voprf` has not moved: 0.6.0-pre.1 (2026-04-06) and
its `main` still pin `p256` 0.13, and 0.5.0 is from 2024, so the PIN
path would keep `crypto-bigint` 0.5 after the openmls bump.

To do: bump `openmls_rust_crypto` and `openmls_basic_credential` to the
release on `p256` 0.14 (watch `openmls` releases after 0.9.2); for the
OPRF either a `voprf` release on 0.14 or our own RFC 9497 client over
`p256` 0.14's `hash2curve` (blind, unblind, finalize — a small module,
checked against the PIN-service interop vectors, §77); then the MLS
conformance tests, and remove the ignore entry.

Addendum 2026-10-31: a third option exists. `voprf-ng` 0.6.0
(2026-10-27, github.com/raphaelrobert/voprf-ng, an openmls maintainer's
fork of `voprf` 0.6.0-pre.1) is on the 0.14 line — `elliptic-curve` 0.14,
`hash2curve` 0.14, `digest` 0.11, `p256` 0.14 in its tests — with the
RFC 9497 vector file identical to upstream's and a mechanical diff
otherwise; `privacypass` 0.6 and phnx-im/air take it. Trust check: a
single maintainer, no release tags, no audit — acceptable for the PIN
path once the openmls crates move, to be re-checked at that bump.

Measured 2026-10-31, as a step candidate (not decided): the forks of
§82 can carry the migration ahead of an openmls release — an openmls
fork branch from the `openmls-v0.9.0` tag with upstream PR #2249 and the
three `hpke-rs` 0.8 lines of #2261 (`openmls_traits` and
`openmls_memory_storage` from crates.io), plus Kupferdraht on `p256`
0.14, `voprf-ng` 0.6 and `rand_core` 0.10 (two code changes: an RNG
adapter for `blind`, `to_sec1_point` for the verifying key), builds,
passes the core tests and the wasm32 check, and leaves no
`crypto-bigint` 0.5 in the lock, so the ignore entry can go. Open risks
before taking it: the live HPKE interop of the classical DHKEM-P256
suite, `ed25519-dalek` 3 on the signing path, and more duplicate crates
in the graph until openmls releases.

## 82) The SQLite3MC fork line: branch policy and the next bump (2026-10-31)

Since Step 127 (ADR-0042) the engine comes from three SchwarzDigits forks
consumed by git tag: rusqlite `v0.37.0-sqlite3mc-2.5.1` (branch
`sqlite3mc/0.37`), refinery `v0.9.1-sqlite3mc` (`sqlite3mc/0.9`), openmls
`sqlite_storage-v0.3.0-sqlite3mc` (`sqlite3mc/sqlite_storage-0.3`). The
openmls and SQLite3MultipleCiphers forks of the organisation are also the
staging area for upstream pull requests, so a long-lived downstream
branch now sits beside the PR heads; the forks' `main` stays identical
to upstream. GitHub's "ahead/behind" on that branch measures against
upstream `main`, while the branch tracks the `openmls-v0.9.0` release
tag, which is not on `main`: seven of the eight "ahead" commits are
upstream's own release-branch commits, and `main`'s `sqlite_storage` is
unreleased code (async storage, rusqlite 0.40, refinery 0.10). The branch
is not rebased onto `main` on purpose; the next branch comes from the
next release tag.

Open: the branch policy is to be confirmed with the maintainer of those
forks; the next bump — when `openmls_sqlite_storage` releases on rusqlite
0.40 — goes rusqlite fork → refinery and openmls branches →
`kupferdraht-core/Cargo.toml` → gate (`vendor/README.md` recipe), and a
rusqlite ≥ 0.38 can retire the fork's wasm arm for its own
`ffi-sqlite-wasm-rs`. (The first Rust embedder dropped its own
`[patch.crates-io]` entry on 2026-11-01.)

## 83) The login path does not enter the manager's client cache (2026-11-01)

Found by Step 78's `forget_account_closes_a_live_client`: only
`Kupferdraht::resume` records a `Weak<Client>` in the manager's cache
(Step 39's "one live `Client` per account"). A client that
`PendingLogin::register_device` or `PendingAccount::activate` returns is the
caller's alone — the manager does not know it. Two consequences:

- `Kupferdraht::forget_account` closes the storage of a client it holds,
  so a client from `login` in the same process keeps working on the
  deleted directory until it is dropped (POSIX unlinks under it; the
  consumer is expected to drop it first, as the TUI's picker does by
  running without a session).
- A `resume` right after `login` in the same process builds a second live
  `Client` for the account — the `zuid` race Step 39's cache was meant to
  prevent — because the cache has nothing to hand out.

The fix is one of: `PendingLogin` / `PendingAccount` get a handle to the
cache (an `Arc` around the map, threaded through `ManagerConfig`) and
insert the client when the registration completes; or `Kupferdraht::login`
returns a manager-aware wrapper. Not done in Step 78 (not needed by it:
every consumer today either runs one process per command or keeps the one
client it logged in with). The Step 78 tests hold their client through
`resume` and say so.
