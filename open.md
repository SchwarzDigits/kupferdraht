# Open — Questions, Backlog & Non-Goals

Everything still on the table. Three kinds of entry: **open questions /
unknowns** (the numbered §N findings log below), **backlog** (decided work not
yet shipped — folded in 2026-06-30 from the former `docs/v1-*-audit.md`), and
**non-goals** (deliberate scope cuts, kept so they aren't re-litigated). When an
item resolves it leaves here — as a permanent Wire/MLS behaviour fact →
`docs/observations.md`, or as a shipped change → `CHANGELOG.md`. Step-sequenced
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

Branch `step/29-client-runtime-split`.

---

## 61) Residuals from the WASM/ADR-0005 divergence finding (2026-07-16; narrowed 2026-09-10)

The core finding is resolved (ADR-0015 supersedes ADR-0005 in
full). Items 1 (THREAT_MODEL leftovers incl. the browser at-rest
model) and 3 (two stale CLAUDE.md stack lines — openmls from
crates.io, the event-stream line vs. shipped ADR-0019) moved to
**Step 82** (`docs/plan/82-threat-model-refresh.md`). Still open
here:

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
- ~~**erg-A.2** `ClientBuilder::production()` / `staging()` presets~~ — decided against 2026-10-08: no preset for Wire's hosted service anywhere, CLI or core; the CLI presets were removed before publication (Wire's Terms of Use restrict third-party clients there).
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
- **gap-B.27** mls.db migrations are not atomic — `openmls_sqlite_storage` 0.3.0 runs refinery non-grouped (`storage_provider.rs:62-65`, not configurable from outside), so a crash between a migration's SQL commit and its history-row commit re-applies it on the next open; V2+ of its migrations are re-apply-fatal. Step 87 closed the same window for kupferdraht.db (`set_grouped(true)`); mls.db waits for the upstream fix (issue draft: `docs/plan/87-atomic-migrations.md`). Blast radius is smaller than kupferdraht.db's: a bricked mls.db can be discarded and the groups rejoined by external commit; the account store cannot.
- **gap-B.29** *(narrowed 2026-09-25, Step 94 live smoke)* The browser login vertical over the OPFS store: **login → registerDevice → syncOnce → rooms → sendText → messages → startSync/onSync ran live** against the own Wire instance from a dedicated worker in headless Firefox (through `bindings/web/kupferdraht-tabs/` from a second tab, over a same-origin dev proxy — observations §87), and after the leader tab was closed the promoted tab re-opened the OPFS store the first worker had written (`accounts()` = 1, then `syncOnce` + `rooms` + `messages`). Still unverified: reload survival of `kupferdraht.db` in a real (non-headless) session and the expiry path (the pre-Step-91 "live browser verifications pending" of Steps 53/55/56/57 are otherwise covered).
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
- **gap-B.16** 4xx httpmock coverage not symmetric (assets / feature-configs / properties / token-refresh covered, not all endpoints). (The Step-73 residual — the legacy drain's error split — is pinned since Step 97b: `step_97b_push_doorbell.rs::a_failing_drain_reconnects_the_socket`.)
- **gap-B.7 residual** — Step 63 shipped pool top-up; the low-pool live drain is untested until a claim source exists (observations §66).
- **gap-B.25 residual** — Step 74 shipped signal-then-flush (`sync/removal_flush.rs`; observations §69/§70). A restart still loses the in-memory pending set; queued proposals persist in openmls storage and commit on the room's next activity.
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
- **gap-B.17** Hand-written migrations only (V1–V22 by hand).

*Defects (wrong references, not shortcuts):*
- **def-D.2** Preview ADR unwritten (NG-2, consumer-demand-triggered); the asset-deletion half is written in **Step 84**.

### Code quality / maintainability

Correct code that is hard to read / navigate. The baseline safety rules (no
`unsafe` / `unwrap` / `println!` / `macro_rules!`; clean build / clippy /
`cargo deny`) are re-checked each gate run — not tracked here.

- **cq-A.1** Files over the 200-code-line cap. Steps 29 / 30 / 32 / 33 retired the four biggest (`client/runtime.rs`, `timeline/send.rs`, `sync/dispatch.rs`, `kupferdraht-cli/src/main.rs`). Remaining offenders **as of the 2026-06-16 sweep — re-count before acting** (Steps 34–42 have since moved code): the three Step-29 residues `client/membership.rs` · `client/rooms.rs` · `client/sync.rs` (see §60), plus `rooms/room.rs`, `client/login_pipeline.rs`, `sync/service.rs`, and pre-existing `wire/http/conversations.rs`, `ffi/timeline.rs`, `wire/dto.rs`, `wire/proto.rs`, `storage/records.rs`, `manager/mod.rs`.
- **cq-A.2** `process_inbound_message` — forbidden `process` prefix. `MlsClient::process_inbound_message` (`encryption/messages.rs`) decodes / absorbs an inbound MLS message → name it for the effect (`decode_inbound_message` / `absorb_inbound_message`).
- **cq-A.3** Three `#[allow(clippy::struct_field_names)]` over renamable fields: `rooms/room.rs` (`room_type`→`kind`), `client/mod.rs` (`mls_client:`→`mls`), `ffi/sync.rs` (FFI `Message` `message_*` fields).
- **cq-A.4** Redundant `_with_storage` suffix on `MlsClient::new_fresh_with_storage` / `resume_with_storage` — pure rename (the in-memory `new_fresh` wraps the `_with_storage` variant; not dead).
- **cq-B.1** Magic byte arrays in fixtures (`vec![0xNN; N]`, no shared names) — introduce named `const`s.
- **cq-B.2** No property-based tests (`proptest` / `quickcheck`) — crypto + protobuf round-trips are candidates.
- **cq-B.3** No benchmarks (`criterion`) — add when perf becomes a question.

### Blocked on the Wire API (no endpoint yet)

- **Step 12 — Key backup (mnemonic):** blocked — Wire v14 exposes no backup endpoint (`docs/observations.md` §62).
- **Step 62 — `/v14/events` push (shipped, dormant):** wire-server
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

---

## Non-Goals — by-design (decided, not open)

> Deliberate scope cuts, kept so they aren't re-litigated. **Not** backlog —
> each is intentional and backed by an ADR / CLAUDE.md / a live-validated
> behaviour note. Reversing one is a scope decision, not a bug fix.

*Backed by an ADR / CLAUDE.md (pointer only):*
- **NG-1** No outbound AES-GCM for assets — AES-256-CBC only; inbound `AES_GCM` decoded but `download()` returns `InvalidInput` (warn-and-skip). ADR-0011; THREAT_MODEL L6.
- **NG-2** No asset preview pipeline — `Asset.Preview` is in the proto, not consumed. ADR-0011.
- **NG-3** Upload retention = `persistent` only. ADR-0011.
- **NG-4** No link-preview generation — `Text.link_preview` echoed verbatim, never fetched. CLAUDE.md.
- **NG-5** No voice / video / calling — `GenericMessage::Calling` skipped. CLAUDE.md.
- **NG-6** No Proteus encryption / decryption — prekeys generated + uploaded (Wire registration contract), never used. ADR-0001.
- **NG-7** No e2ei certificate enrollment — leaves advertise `X509` (W-7) but Kupferdraht never enrolls; BasicCredentials path only. CLAUDE.md.
- **NG-8** No user-property writes — `GET /properties` only; config awareness is read-only. ADR-0012.
- **NG-9** No login refusal on hostile feature policy — feature-config fetch is warn-and-continue. ADR-0012.
- **NG-10** No federation across non-Wire backends — single deployment domain. CLAUDE.md.
- **NG-11** External-senders extension = operator trust — the backend has membership-administration authority (Remove proposals) but never content; MLS-standard, visible to all members. ADR-0004 (W-5).
- **NG-12** Asset authenticity from MLS, not local AEAD — AES-256-CBC (unauthenticated) + a SHA-256 carried inside the MLS-encrypted `GenericMessage`. ADR-0011.
- **NG-13** `attachments` FK `ON DELETE NO ACTION` — deliberate, to avoid an FK-cascade history wipe. CHANGELOG.md §32.

*Standalone design notes (no ADR — kept so the rationale survives):*
- **NG-15** macOS keychain re-prompts per fresh build — dev-time only; `KUPFERDRAHT_DEV_KEYSTORE` is the workaround.
- **NG-16** Inbound / download cache filename carries `message_id` (`{message_id}.enc`); the send-side temp uses a random name.
- **NG-17** *(Step 79, 2026-09-24)* No resend of task / case / marker snapshot rows and no background resend scheduler — `Timeline::resend` refuses full-state snapshots (a stale replay can regress every receiver's LWW fold; re-send the current state via `update_task` / `set_case` / `set_task_list`), and retrying after an outage is the consumer's loop (`docs/library.md` "Reconnect contract"). ADR-0014 / ADR-0018 / ADR-0020; plan `docs/plan/archive/79-failed-send-recovery.md`.

## 64) Chat-system basics — the post-Step-60 gap map (2026-07-19)

Roadmap discussion after tasks shipped: what a daily-driver messenger
still needs. Backlog candidates, grouped; PLAN.md steps get generated
from here with the user's OK. Items already tracked elsewhere are
referenced, not duplicated.

### Safety & contacts (the biggest true gap)

- **Connections / block** — Wire's `/connections` model (connect,
  accept, **block**) is entirely absent. Blocking is a baseline
  safety feature, not polish.
- **User search** — no `/search/contacts` (find by handle); without
  it a user cannot *start* a conversation with someone new.
- **Device verification UX** — MLS gives cryptographic identity, but
  no fingerprint/verification surface in the library yet.

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
- **Link previews** — Wire's `Text` carries asset-based preview
  fields; visible polish, not structural.

### Mobile viability

- **Push registration** (APNs/FCM, `/push/tokens`) — *updated
  2026-09-10:* no longer the hard blocker as written — the team
  redirects Wire's push call to its own backend (2026-08 decision),
  so mobile wake-up rides that path; a Kupferdraht-side `/push/tokens`
  surface is only needed if the stock Wire push path ever comes
  back into play.
- **New-device history** — systemic MLS boundary (no server archive);
  the task join snapshot (ADR-0014 decision 6) is the task-side
  answer, message backup is Step 12 (no endpoint in Wire API v14). The limit
  should at least be *communicable* to consumers.

### Group governance (second tier)

- **Conversation roles** — *narrowed by Step 72 (2026-08-21):*
  roles ARE decoded, persisted (`room_members.role`, V26), merged
  through `sync_members`, and honored where authorization exists
  (the ADR-0018 case gate, `[admin]` in `room-members`). Still
  open: invite/kick are role-blind — deliberately so far, since
  Wire itself lets any member invite (observations §72); gating
  them would be a Kupferdraht-side policy decision, not parity.
- **Room avatars** — absent.

### Deliberately out (charter, unchanged)

Voice/video calls, e2ei enrollment, Matrix-sense threads, JS port.
Presence ("online") — Wire barely has it, privacy stance; leave it.

### Priority thesis

1. ~~**Self-conversation step** (§63)~~ — **shipped as Step 86
   (2026-09-24)**: the channel exists; the triple leverage (personal
   task list, `LastRead` cross-device read sync, draft sync) is now
   three follow-ups under Proposed steps.
2. **Connections/block + user search** — safety and "start a chat".
3. **Mute/archive + unread counts** — daily comfort.
4. **Push** — as soon as the bindings get serious (Step 20e).
5. Local search as a filler between steps (ephemeral toleration
   shipped with Step 89, 2026-09-17).

## 66) External code review triaged — 18 findings verified, dispositions assigned (2026-08-13)

An external review (18 findings, S1–S18, re-checked against
`f17abbd`) was verified finding-by-finding against the source.
Result: 15 confirmed, 3 partially confirmed (mechanics right, impact
overstated), 0 refuted. The review's one systematic gap: it does not
follow the MLS consequence chains to the end — Step 38's
`recover_room` self-healing and MLS's skipped-generation tolerance
soften S1 ("critical" → high: messages lost until heal, no permanent
ratchet brick) and S3 (send failure is caller-visible, receivers
tolerate generation gaps — no action needed).

Dispositions (what is still open):

- **Shipped:** Steps 64 / 65 / 66 (see `docs/plan/archive/`); S15
  docs-vs-code (chore on the triage branch); S9 as Step 71
  (gap-B.20); S17 as Step 70 (gap-B.21).
- **Shipped 2026-09-25:** S18 → gap-B.9 as Step 85 (ADR-0024). **Still open:** S11
  (task-cache monotonicity) is a design question — ADR-0014's
  last-writer-wins means *event order*; a wall-clock guard would
  change semantics; revisit if cursor-reset replays become a real
  flow.
- **No action:** S3 (documented, caller-visible, MLS-tolerated),
  S14 (IndexedDB task no-ops — ARCHITECTURE §7).


## 68) `invite_members` freshness is a contract, not enforced (2026-08-26)

The ADR-0020 invite hook hands the invitees this client's *current*
fold of case/marker/tasks — state the inviter has not drained is
permanently missing for the joiner (MLS has no history). Live-caught
in the Step-75 smoke: the one-shot CLI invited before folding a
pending task; the late joiner's board was short. Fixed for the CLI
(`room-invite` now runs a full `sync_once` first) and documented as
a contract on `invite_members` (push-loop consumers are always
fresh). Open decision: should `invite_members` drain notifications
itself (core-side enforcement — costs a sync round-trip on every
invite, duplicates the consumer's loop) or stay a documented
contract? Revisit if a second consumer trips over it.

## 69) Kalium-audit residuals — checked, accepted, not commissioned (2026-09-10)

The Kalium-JVM production-readiness audit was cross-checked against
Kupferdraht (Steps 77–80 commissioned from the real findings; the
blocker class does not transfer — one Rust core, SQLCipher
everywhere, calling out of scope). Three minors were verified and
deliberately left as residuals rather than steps:

- ~~No `busy_timeout` / `synchronous` pragmas~~ — resolved by Step 85: both set explicitly on every native open path (observations §84: the busy_timeout residual never existed as stated).
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

Step 95c turned every assertion on runtime data into a typed error
(`docs/plan/archive/95-inbound-message-integrity.md` "As built"). Left open —
the first two deliberately, as plan D4 foresaw:

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

A production-readiness review commissioned by Markus (four parallel
code audits — MLS/crypto, local secrets, network/sync,
state/concurrency — with every blocker and high finding read in the
code) concluded: **not production
ready as a security product in the sense of THREAT_MODEL.md.** The base
is sound (architecture, openmls without unsafe settings, verified DB
keys, rustls-only, atomic migrations, Step-80 path hardening).
Recommended order: blockers 1–3 → 4–5 → 6–8 → transport → threat model
and process.

**Done by Step 95:** blockers 1–3 (MLS-authenticated sender, ADR-0027;
sender-bound edit/delete, ADR-0028; no panics on foreign data), plus
three findings 95c fixed on the way — conversations listed in 1000-id
chunks, asset URL path segments validated, runtime-data asserts
(e.g. resume after `SessionExpired`). The ghost-device risk is now at
least visible (95d, room devices + MLS thumbprints).

**Done by Steps 96–106** (merged 2026-09-28/29, archived under
`docs/plan/archive/`, recorded in `CHANGELOG.md`): B5 (Step 96,
ADR-0029), B6 (97b, ADR-0030), B7 (97a), B8 (97c, 97d — ADR-0031), B9
(97e, ADR-0032; the two-write events by Step 105), H1 (98), H2 (99), H3
(102), H4 (106, ADR-0034), H6 (101), H7 part 1 (104), H8 (103; push as
the default where a target has one, ADR-0033).

**Still open** (each re-verified at `5f7eef4`):

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
- **H7 part 2** — browser: the OPFS pool is fixed at 24 files — per its
  own comment sized for about two accounts, open.md earlier said about
  ten; measure before fixing.

*Medium / low:*
- **M1** — WebSocket: no connect timeout; pongs are ignored (liveness
  rests on a failed ping send).
- **M2** — redirects: reqwest's default policy follows https→http, and
  on 307/308 re-sends the login body with the password to another host.
- **M3** — `ws://` is accepted without `permit_http`.
- **M4** — Windows: `is_path_safe_id` passes `:`, reserved names
  (`CON`, `NUL`) and a trailing dot or space.
- **M5** — `global.db` has no schema version; logout leaves all local
  data (Step 78, known); `PRAGMA secure_delete` is set nowhere.

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

*Operations and process:* `SECURITY.md` is the unedited GitHub template
(no reporting channel, no external audit); no fuzzing, no property
tests, no automated interop tests against Wire; CI runs on Ubuntu only,
without the Kotlin / Swift binding tests, on an unpinned toolchain (no
`rust-toolchain`, no `rust-version` — clippy `-D warnings` verified green
on stable 1.98 on 2026-09-28, so an older or newer stable may differ
without anyone noticing);
0.1.0 without a release or API-stability promise (Step 20e); licensing
for proprietary consumers (GPL-3.0-only; contributions come in under the
Schwarz Digits CLA) and the Wire ToU question are business decisions.

## 72) Browser server mode — residuals of Step 111b (2026-09-30)

What Step 111b (ADR-0035, the account databases on a storage server) left
open, none of it blocking the step:

- **A long pending login.** Until `registerDevice` returns, the storage
  token is renewed with the login's Wire token, which expires after
  15 minutes; a login left pending longer retries the renewal every minute
  until the client exists. The first renewal is due after half the
  storage token's lifetime (30 minutes), so this only matters for a login
  left open that long.
- **A purge needs a live client (Step 114).** `logout({ purgeLocal })`
  runs on a `Client`: an account whose session expired is logged in again
  (its keys are here), then logged out with the purge. A manager-level
  purge by account id — Step 78's `destroy_account` — would spare the
  login; it is the native purge's design question, not the browser's.
- **One login cookie per backend.** The browser keeps one `zuid` per
  backend domain: in the server mode `resume` of an account other than the
  last one logged in is `SessionExpired` (the cookie names another user).
  Not planned (Markus, 2026-09-30): the browser deployment has one account
  per browser; revisit only if several accounts on one backend become a
  use case.

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

## 78) Publication snapshot — checklist (2026-10-08)

The repository goes public as a single snapshot (README "License");
the development history stays private. These steps happen when the
snapshot is built, in this order, and each is checked off before the
public repository receives its first push.

- [ ] **Build from a clean clone of `main`**, never from a working copy:
      `git clone` into a fresh directory, `git checkout --orphan public`,
      `git add -A`, one commit. `git rev-list --count HEAD` must print `1`.
      A clean clone guarantees that ignored files (`scripts/smoke.env`,
      `reference/wire.swagger.json`, `.claude/settings.local.json`,
      `target/`) cannot leak in.
- [ ] **Leave `.mailmap` out** (`git rm .mailmap` before the commit). It
      maps the placeholder identity of one private-history commit to a
      maintainer's internal address; without the history it is useless and
      the address is the only internal one in the tree.
- [ ] **Grep gates on the snapshot tree** (`git grep -IEn`, `vendor/`
      excluded): the retired device identity — the user-id prefix and the
      client id kept in the maintainer's private notes, never quoted here —
      must have no hit at all. `onstackit|digits\.schwarz|mail\.schwarz`
      (internal hosts and mail domains) may hit only the program's public
      mailbox `opensource@digits.schwarz`. The sister project's former
      name (kept in the maintainer's private notes, never spelled here)
      must have no hit; `wire://sso-login` may hit only this list. `/Users/|/home/` may hit
      only the container path in `.github/scripts/prepare-compliance.sh`
      and the fixture paths in the `storage` tests. `BEGIN
      (RSA|EC|OPENSSH) PRIVATE KEY` and `eyJ[A-Za-z0-9_-]{20,}` (JWTs)
      must be empty. `sodium` may remain in the SHA-pinned SQL migrations
      (`tests/migration_immutability.rs`), in the rename record (ADR-0041,
      its rule, CHANGELOG, LEARNINGS) and in ADR-0006's mention of
      libsodium; nowhere else. Measured 2026-10-09 on a dry run of this
      procedure: exactly these hits.
- [ ] **Author of the snapshot commit** is the maintainer's own identity,
      the one the retained history carries, with the name spelled as in
      the README copyright line. The copyright holder named there is
      Schwarz Digits KG whoever authors the commit. Signed with the SSH
      key registered on the maintainer's GitHub account, so the commit
      shows as verified.
- [ ] **Dangling pointers are accepted, not fixed:** the archived plan
      headers name commit SHAs of the private history, and PLAN.md cites
      PR numbers. The README says the history is retained privately and
      answers provenance questions; nothing else needs rewriting.
- [ ] **Regenerate `docs/THIRD_PARTY_LICENSES.html`** against the final
      `Cargo.lock` (`cargo about generate about.hbs`) and refresh
      `THIRD-PARTY-NOTICES` (command in its last section) — or confirm
      with `git log` that `Cargo.lock` has not changed since both were
      generated. The five gates plus the two wasm32 checks must be green
      on the commit the snapshot is built from; CI on `main` counts, the
      tree is identical but for `.mailmap`.
- [ ] **GitHub settings, in this order:** the empty repository has
      `main` as default branch and no Actions secrets or variables. Push
      the snapshot, confirm that license detection shows GPL-3.0 and that
      the first `Compliance` run is green, then make the repository
      public. Only then add the ruleset protecting `main` (natrium's
      "protect main" is the model) with `Compliance` as a required check:
      on this plan GitHub refuses rulesets and branch protection while the
      repository is private. The CLA Assistant is installed for the
      organisation and reports as the `license/cla` check on the first
      pull request; nothing to configure per repository. Security reports
      go to the program's address (SECURITY.md), so no repository-level
      advisory setup is needed.
- [ ] **After the push:** `cargo deny check` and CI green on the public
      repository; the README's build instructions tried once from a clean
      machine without access to any Schwarz infrastructure. The first
      `Compliance` run reviewed and then made a required status check.
      Expected, as measured on the private `main` on 2026-10-08: no
      forbidden license, no copyleft warning, and 30 "unclassified
      license" hints over six licenses — Unicode-3.0 (the icu family,
      19 crates), Apache-2.0 WITH LLVM-exception, BSD-1-Clause,
      CDLA-Permissive-2.0, BSL-1.0, and NOASSERTION on the test-only
      `stringmetrics`, whose license file is Apache-2.0 — unless the
      central classification has been extended by then (§79). The
      Gitleaks allowlist (`.gitleaks.toml`) covers the interop vectors and
      the planted fake token, verified against the full history. The ORT
      scanner stays disabled until the central workflow works with ORT 96
      (§79).

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

## 80) Provenance audit after counsel's opinion (2026-10-20)

Counsel's second opinion (2026-10-20) confirmed the licence position
(GPL-3.0 on both sides; the GPL-3.0-or-later schema combines into the
GPL-3.0-only work; the licence agreement for the Kalium fork does not
reach Kupferdraht) and asked for three things before publication: the
documented Kalium readings complete, NOTICE and README naming every
source, and a sample-based structural comparison of the code against
Kalium.

Done on branch `chore/provenance-audit-fixes`:

- **Structural comparison** of the whole tree against wireapp/kalium at
  9ccecabb (automated layer with matrix-rust-sdk as the independent
  baseline, plus eight module-pair readings). Record:
  `docs/wire/provenance-audit-2026-10.md`. One distinctive item
  (`FileSharingPolicy`'s three-state model, ADR-0012 revision 1.3), two
  name-only echoes, one field order, one CLI default — all removed.
- **Readings index** `docs/wire/kalium-readings.md`: every reading with
  file and permalink, what it informed, what was replaced (the quote-hash
  conformance vectors, the file-sharing model, the April 2026 WebSocket
  prototype) and two attribution corrections.
- **Wording**: README provenance rule, CLAUDE.md goal and NOTICE
  ("Sources") say the same thing; the proto header dates its
  modifications (GPL-3.0 §5(a)).

Still to do before the snapshot is rebuilt: nothing in the tree. Then
the §78 procedure again (delete and recreate `SchwarzDigits/kupferdraht`,
fresh orphan commit).

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
