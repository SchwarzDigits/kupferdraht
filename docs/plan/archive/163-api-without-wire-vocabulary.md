# Step 163 — The public API in matrix-rust-sdk's vocabulary

**Status: MERGED 2026-11-22**, in five sub-steps, each its own branch, commit and PR:
163a `9565c8b` (PR #106, `abfcd58`, 2026-11-18: ADR-0009 revision 1.2 and the check
`tests/public_vocabulary.rs`); 163b `a32a6f0` with the fix-ups `5f1d4b7` and `f82f90c`
(PR #110, `851e546`, 2026-11-18: the message kinds); 163c `18e88b3` (PR #111,
`b309403`, 2026-11-18: the rooms); 163d `2b10240` (PR #112, `d99cd31`, 2026-11-22:
identity, devices, the session; smoke on server 4 the same day, 33 ok over two runs);
163e `01c0b26` (PR #116, `9d73096`, 2026-11-22: the server's settings, the account
data, the store contract; locally fmt, 0 build warnings, clippy all targets, the full
core and CLI suite with CI's profile — 175 binaries, 1798 passed — and the pins 8/8 +
9/9; CI green on the first run). The smoke after 163b–e ran on server 4 (Wire 5.38.9)
on 2026-11-22: the regular smoke, the 163d block and the 163b–e block (ping, a
disappearing text, room access and a join link joined by external commit, the
personal room, the settings, the account data), 48 ok, 0 failed. BREAKING across
Rust, FFI, browser, the store contract and the CLI, listed in the CHANGELOG; the
check's temporary list is empty. Retrospectives: LEARNINGS 5.104 (163b, 163c), 5.105
(163d), 5.106 (163e). ADR-0009's part 4 was reworded as a preference on 2026-11-22
(revision 1.3, PR #115).

Commissioned 2026-11-17 (Volker), from an audit of the public API the same
day. ADR-0009 and its rule say the public API speaks matrix-rust-sdk's language: its
concepts outside `wire/`, Wire's field names inside it. The audit found about
fourteen families of public names (Rust, UniFFI, browser) that carry Wire's words or
Wire's raw values. Most of them are recorded in `docs/api-mapping-matrix-rust-sdk.md`
as "Wire extension" or "Drift", some were settled by ADRs (0004, 0011, 0012, 0025).
Volker's decision: no exception for concepts matrix has no counterpart for — these get
neutral names too. Compatibility stays where it belongs: in `wire/`, which keeps Wire's
names and maps them.

## Problem

Wire's words and values in the public surface, grouped (Rust / Kotlin–Swift / browser
names; the audit's file references are in its report of 2026-11-17):

- **Message kinds.**
  - `Knock` / `hot_knock` (`TimelineItemKind::Knock`, `MessageBody::Knock`,
    `send_knock`, `sendKnock`, CLI `knock`). In matrix, *knock* means asking to
    join a room, so the name misleads.
  - `send_ephemeral_text` and `ephemeral_ttl_ms`. In matrix, *ephemeral* means
    typing and receipt events.
  - `Asset` for files: `MessageBody::Asset`, `TimelineItemKind::Asset`,
    `Timeline::asset()`, `AssetSummary`, the FFI `Asset` object, `AssetSource`,
    `downloadAsset`, `kind: "asset"`. matrix says media and attachment.
- **Wire's locator and key material in public records.**
  - `asset_id`, `asset_token`, `asset_domain` and `encryption_algo` on
    `AssetSummary` and the FFI `AssetMeta`.
  - `otr_key_ct` on the store's `AttachmentRecord`.
  - The browser's `AssetMeta` already leaves them out.
- **Room access.**
  - `access_modes()`, `access_roles()`, `access() -> RoomAccess{modes, roles}`,
    `set_access(access, access_role)`, `Client::set_room_access` and
    `roomAccess` / `setRoomAccess`.
  - These take Wire's field names and Wire's raw strings (`"invite"`, `"code"`,
    `"team_member"`, `"guest"`, …) as `Vec<String>`.
  - The mapping doc's parameter names (`modes, roles`) do not match the code.
  - matrix has a typed `JoinRule` and `GuestAccess`.
- **Rooms.**
  - `join_via_link(code, key, password)` takes Wire's `ConversationCode` fields,
    while `create_join_link` returns a URL.
  - `self_room()` and `RoomType::SelfRoom` come from Wire's "self conversation".
- **Identity, devices, session.**
  - `QualifiedUserId` is the FFI's identity record, while the core type is
    already `UserId`.
  - `DeviceInfo{class, label, model, last_active, registered_at}` uses Wire's
    client-object fields, with ISO strings instead of `*_ms`. Related:
    `device_label` and `rename_device(id, label)`.
  - `cookie_label`.
  - `handle`, `set_handle` and `RoomMember.handle`.
  - `is_self` on members, devices and reactions.
  - `avatar_key`, `AvatarKeys`, `avatar_thumbnail_key`, `avatar_full_key` and
    `team_avatar_key` hold Wire asset keys.
  - `thumbprint` on devices and `own_device_thumbprint`.
  - `backend_removal_key_ecdsa_p256`. RFC 9420 calls it the external sender.
- **Server settings.**
  - `feature_configs()`, `feature_config(name)` and `FeatureConfigEntry`, raw by
    Wire's feature keys.
  - `user_properties()`, `user_property(key)` and `UserPropertyEntry`.
  - `ReceiptMode` and `receipt_mode()`.
- **The custom-store contract** (public through `StorageProvider`):
  - `DeviceRecord.client_id`, while `SessionRecord` says `device_id`.
  - `SyncStateRecord.sync_marker_uuid` and `DeliveryReceiptRecord.confirmer_user_id`.
  - `BackendMlsKey{purpose = "removal"}`.
  - The `Store` methods named after feature configs, user properties and
    ephemeral expiry.
- **Prose a consumer reads.** The JSDoc that wasm-bindgen copies into the `.d.ts`
  ("conversation", "qualified Wire id", "Wire `client_id`"). The Display texts of
  `MlsGroupMissing` and `MlsStaleMessage` ("conversation").

## Scope

The rule first, then the renames in sub-steps, each its own branch, commit and PR. The
names below are proposals; phase 1 of each sub-step fixes them against matrix-rust-sdk
where it has a term, otherwise a plain English name.

- **163a — the rule and the ADR.**
  - One ADR (number reserved when the sub-step starts) states the line. Wire's
    words and raw Wire values stay in `wire/`. A concept matrix lacks gets a
    neutral name. A raw accessor exposes neither Wire's keys as names nor Wire's
    strings as values without a typed projection.
  - It revises the naming parts of ADR-0004, ADR-0011, ADR-0012 and ADR-0025, and
    tightens `rules/0009`.
  - The "Wire extension" rows of the mapping doc are rewritten as each sub-step
    lands.
  - **Done 2026-11-18 (Volker):** no new ADR — the line follows from ADR-0009, so
    it gets revision 1.2 (part 4: names, values, prose, and Wire's names only where
    an interface to Wire binds them, approved beforehand and marked with
    `Wire compatibility (ADR-0009)`). `rules/0009` tightened; revision entries in
    ADR-0004 (1.1), ADR-0011 (1.4), ADR-0012 (1.4), ADR-0025; `rules/0012` and
    `rules/0025` name their leaving names. The check:
    `kupferdraht-core/tests/public_vocabulary.rs` — 29 words, a temporary list of
    today's 184 occurrences (file × word) by sub-step (163b 44, 163c 12, 163d 91,
    163e 37), an empty list of approved spots. Backticked citations of Wire's
    interface (endpoint paths, event types, status labels) stay in prose.
  - **Found by the recount, for the sub-steps:** `open_to_guests` /
    `allowGuests`, `RoomType::Channel` and the parameters `code` / `key` of
    `join_via_link` (163c); `DeviceInfo`'s `class` / `label` / `model` (163d,
    too generic for the lexical check); `SelfUserRecord`, `WIRE_RECEIPT_MODE` and
    the stored kind values `"knock"` / `"asset"` / `"self_room"` (163b, 163c, 163e
    decide in phase 1 whether stored values change).
- **163b — message kinds.**
  - `Knock` becomes a ping or nudge (`send_ping`, `TimelineItemKind::Ping{…}`), and
    `hot_knock` gets a plain flag name.
  - Ephemeral becomes disappearing: `send_disappearing_text`,
    `disappearing_ttl_ms`.
  - `Asset` becomes file or attachment: `MessageBody::File`, `Timeline::file()`,
    `FileSource`, `downloadFile`, `kind: "file"`.
  - Wire's locator and key fields leave the public records behind an opaque
    handle, as the browser already does.
  - **Done 2026-11-18 (Volker's names):**
    - Knock → **Ping**: `send_ping(repeated)`, `Ping { repeated }` in
      `MessageBody`, `TimelineItemKind` and `OutgoingContent`; browser
      `sendPing`, the item's `repeated`; CLI `ping --repeated`.
    - Asset → **Attachment** (matrix-rust-sdk's word): `Timeline::attachment`,
      `AttachmentSummary`, `AttachmentSource`, `MessageBody::Attachment` /
      `TimelineItemKind::Attachment`, FFI `Attachment` / `AttachmentMeta`,
      browser `downloadAttachment` and `kind: "attachment"`, CLI
      `download-attachment`; `send_file` / `send_file_stream` / `sendFile` stay.
      `AttachmentSummary` and `AttachmentMeta` drop the locator, the token and the
      cipher (`AttachmentSummary` keeps them `pub(crate)` for the download).
    - Ephemeral → **disappearing**: `send_disappearing_text`, CLI `send --ttl`,
      `Store::expire_disappearing_messages` / `pending_disappearing_expiries`.
    - The store contract: `TimelineEventRecord.ttl_ms`; `AttachmentRecord`
      `key_ct`, `attachment_id` / `attachment_token` / `attachment_domain`,
      `cipher` — serde names follow, BREAKING. The SQL columns stay.
    - The stored kinds `"knock"` / `"asset"` become `"ping"` / `"attachment"`:
      migration V37, the only place besides its test that still names the old
      values.
    - The module files follow (`timeline/ping.rs`, `disappearing.rs`,
      `attachment_*.rs`, `encryption/attachment_crypto.rs`,
      `sync/dispatch/{ping,attachment,disappearing}.rs`, `cli/attachments.rs`),
      the tests too. The words inside `wire/` and the Protobuf schema are Wire's,
      by rule; an avatar's keys are image keys (their names: 163d).
    - Pins: `tests/step_163b_message_kinds.rs` (the description's fields), the V37
      test in `storage/schema.rs`, the renamed browser pins of Steps 98c, 130a and
      130c; Step 132's token warning went with the token.
    - The account's cache directory is `attachments/` (Volker: no step moves an
      existing `assets/` — Kupferdraht is not in production use yet). Raised in
      review: nothing would reach the old directory any more, copies of files
      deleted for everyone included; so a client's start removes it whole
      (`a_start_removes_the_cache_directory_of_an_earlier_release` in
      `tests/step_84_deleted_file_is_gone.rs`).
    - Left: `docs/THREAT_MODEL.md` keeps "asset" in L6 and the trust model.
- **163c — rooms.**
  - Room access is typed: a join rule (invite only, by link, …) and who may join
    (team members, guests, services). Wire's strings are mapped in `wire/`.
  - `join_via_link(url, password)` takes the link that `create_join_link` returns.
  - `self_room` gets a neutral name (for example a personal room).
  - **Done 2026-11-18 (Volker's names):**
    - `RoomAccess { join_rules: Vec<JoinRule>, allowed: Vec<Joiner> }`:
      `JoinRule { Private, Invite, Link, JoinLink }` (matrix-rust-sdk's name;
      Wire's `code` is the room's `JoinLink`, Volker considered `AccessLink`
      too — `JoinLink` matches the record and `create_join_link`),
      `Joiner { TeamMembers, NonTeamMembers, Guests, Services }`. Sets, as the
      backend keeps them; both of its sets are closed, so every state it can
      hold has a value — the round trip over all 16 × 16 combinations is a unit
      test (`rooms/access.rs`). Wire's strings are `wire::dto::{Access,
      AccessRole}`, the rooms layer maps the variants.
    - `Room::cached_access` (was `access_modes` / `access_roles`),
      `set_access(RoomAccess)`, `allow_guests` (was `open_to_guests`; the
      browser's `allowGuests` already was the name).
    - `join_via_link(url, password)`: the URL is split in `wire/`; one that is
      no join link is `InvalidInput` before any request, naming no secret. The
      CLI reads the URL like the key before it: TTY or `--url-stdin`, never argv.
    - `RoomType::Personal`, `Client::personal_room`, CLI `personal-room`; the
      stored `"self_room"` becomes `"personal"` (migration V38). `Channel`
      stays (Volker: plain English). Wire's `ConvType` variant is
      `SelfConversation` inside `wire/`.
    - Pins: `tests/step_163c_room_names.rs`, the V38 test in
      `storage/schema.rs`; `tests/step_86_self_conversation.rs` is
      `step_86_personal_room.rs`.
- **163d — identity, devices, session.**
  - `QualifiedUserId` becomes `UserId`.
  - `DeviceInfo{kind, display_name, model, last_seen_ms, registered_at_ms}`,
    `initial_device_display_name`, `rename_device(id, display_name)`.
  - `cookie_label` becomes `session_label`, `handle` becomes `username`, `is_self`
    becomes `is_own`.
  - Avatar keys become an avatar source.
  - `thumbprint` becomes `fingerprint`, `backend_removal_key_ecdsa_p256` becomes
    `external_sender_key`.
  - **Done 2026-11-19 (Volker: "alles wie vorgeschlagen, Avatar (b)"):**
    - The FFI record `UserId { localpart, domain }` (core `UserId` imported
      as `CoreUserId` in `ffi/`). The protobuf assignee is built by
      `wire::proto::user_reference`, so its type name stays in `wire/`.
    - `DeviceInfo { device_id, kind: DeviceKind, display_name, model,
      registered_at_ms, last_seen_ms, is_own }`, `DeviceKind { Phone, Tablet,
      Desktop, LegalHold, Other }`: Wire's classes are
      `wire::dto::ClientClass`, the device layer maps them; no class or an
      unknown one is `Other`, the times read with `wire::time::parse_time_ms`.
    - `ClientBuilder::initial_device_display_name`, `rename_device(id,
      display_name)`; `username` / `set_username` (Account, the FFI record,
      `AccountEntry`, `RoomMember`, the browser's getter and `setUsername`);
      `is_own` (members, devices, reactions); the tabs' leader event says
      `isThisTab` — `isOwn` does not fit a tab.
    - Avatar (b): `AvatarSource { thumbnail, full }` (serde and UniFFI),
      `Account::avatar_source`; the version marker is `avatar_version`
      (`Client`, `RoomMember`, browser `avatarVersion`) and
      `team_avatar_version`; `login_avatar_keys` (deprecated since 0.5.1) is
      removed rather than renamed.
    - `external_sender_key` — the MLS term (RFC 9420's `external_senders`,
      openmls' `ExternalSendersExtension`); `session_label` (the CLI's
      `status` prints `session_label` and `stored_session_label`).
    - `thumbprint` stays (Volker, 2026-11-19, after the implementation): it
      is RFC 7638's name for the computation (JWK Thumbprint), which Wire took
      over — neither MLS nor openmls name it, and a standard's term is no Wire
      word. The word left `tests/public_vocabulary.rs`; the code keeps
      `own_device_thumbprint` and `RoomDevice::thumbprint`. Checked at the
      same time against openmls (sources and book): no other 163b–d name
      replaced an MLS term; `MlsClient` keeps MLS's "client", and the
      renamed `client_id` was always the backend's device id.
    - Internally every `client_id` outside `wire/` is `device_id` — the
      recovery data's field names included, its format unchanged; the SQL
      columns and the backend's query parameters keep theirs. The mention
      resolver speaks of usernames.
    - The store records' serde names follow; the columns stay, nothing
      migrates.
    - CLI: `device-rename --name` / `-n`, `profile-set --username`; the help's
      "qualified" ids are plain ids, the `zuid` the refresh cookie.
    - Pins: `tests/step_163d_identity_names.rs` (15), the `ClientClass` round
      trip in `wire/dto.rs`; `tests/step_125_client_handle.rs` is
      `step_125_client_username.rs`.
    - Merged 2026-11-22 (#112, main `d99cd31`; the step's commit `2b10240`).
      Smoke on server 4 (Wire 5.38.9, the second Haskell reference machine) the
      same day: the regular smoke plus the 163d block, 33 ok, 0 failed over two
      runs — the second after users 2 and 3 got a username, so
      `profile-set --username` there and back ran live too. The 163b/163c
      features (ping, disappearing message, room access, join link) wait for
      the smoke after 163e, as sketched below.
- **163e — server settings and the store contract.**
  - Feature configs become typed settings, with the raw map internal or
    neutrally named.
  - User properties become account data, `ReceiptMode` a read-receipt setting.
  - The store's records and methods follow, so the Rust names change and the SQL
    columns stay.
  - The JSDoc and the error texts are reworded.
  - Standard terms stay (Volker, 2026-11-19): `BackendMlsKey` holds the
    backend's external sender key, so its neutral name is MLS's
    ("external sender key"), not a new one; `signature_scheme` in it stays
    (TLS / RFC 9420). The check's word "ephemeral" (Wire's ephemeral
    messages, 163b) must not hit the cryptographic term (an HPKE ephemeral
    key): the matcher learns that before 163e relies on it.
  - **Done 2026-11-22 (Volker: "as proposed; the raw accessor stays public; the
    matcher reads prose word pairs; `purpose` is kept"):**
    - Server settings: `Client::server_settings()` / `server_setting(name)` and
      the record `ServerSetting` (same fields as `FeatureConfigEntry`); the raw
      accessor stays public, beside the typed `file_sharing()`. The module
      `client/server_configs.rs` is `server_settings.rs`.
    - Account data: `Client::account_data()` / `account_data_entry(key)` and
      `AccountDataEntry` (same fields as `UserPropertyEntry`); matrix-rust-sdk's
      `Client::account().account_data::<T>()` is the nearest concept.
    - `ReadReceiptSetting { Off, On }` and `Client::read_receipt_setting()`
      replace `ReceiptMode` / `receipt_mode()`. `file_sharing()`,
      `FileSharingPolicy` and `server_supports_push_events()` stay.
    - The store contract: `ExternalSenderKey` (MLS's term, RFC 9420's
      `external_senders`; the field `purpose` stays) replaces `BackendMlsKey`,
      `OwnUserRecord` replaces `SelfUserRecord`, `SyncStateRecord.sync_token`
      replaces `sync_marker_uuid`, `DeliveryReceiptRecord.reader_user_id`
      replaces `confirmer_user_id`; the `Store` methods follow
      (`save_` / `load_external_sender_key`, `save_` / `load_own_user`,
      `replace_server_settings` / `upsert_server_setting` /
      `list_server_settings` / `get_server_setting`, `replace_account_data` /
      `upsert_account_data_entry` / `delete_account_data_entry` /
      `list_account_data` / `get_account_data_entry`). The records' serde names
      follow the Rust names (BREAKING for a custom store that persists the
      JSON); the SQL tables and columns (`feature_configs`, `user_properties`,
      `backend_mls_keys`, `self_user`, `sync_state.sync_marker_uuid`,
      `delivery_receipts.confirmer_user_id`) keep their names, nothing migrates.
    - FFI: the same method names, the records `ServerSetting` and
      `AccountDataEntry`, the enum `ReadReceiptSetting`. Browser:
      `serverSettings()`, the `.d.ts` interface `ServerSetting`, the tabs
      protocol and proxy.
    - CLI: `settings [--name <key>]` (was `features`), `account-data [--key
      <key>]` (was `properties`).
    - The error texts of `MlsGroupMissing` and `MlsStaleMessage` and the
      JSDoc say "room" where they said "conversation".
    - The check: its temporary list is empty; the matcher finds a
      multi-segment word written with a space in prose too ("feature config"),
      treats "ephemeral key" (HPKE) as the cryptographic term, not Wire's
      ephemeral message, and reads a backticked Rust path (`Self::room`) as
      code, not prose.
    - Prose: the doc comments, help texts and consumer docs (`docs/library.md`,
      `docs/cli.md`, README) say "room" where they said "conversation", and
      the internal names follow (`own_user_id`, `sync_token`, `reader`, the
      settings dispatch handlers); Wire's event types, endpoint paths and DTO
      field names stay as citations.
    - Pins: `tests/step_163e_settings_names.rs` (8), the two matcher tests and
      the empty-list test in `tests/public_vocabulary.rs`;
      `tests/step_98d_wasm_feature_configs.rs` is
      `step_98d_wasm_server_settings.rs`.
    - Smoke on server 4 (Wire 5.38.9), 2026-11-22: the regular smoke, the 163d
      block and the 163b–e block (ping, a disappearing text, room access and a
      join link joined by external commit, the personal room, the settings and
      the account data), 48 ok, 0 failed; both devices purged.
- **Every sub-step:**
  - FFI and browser parity, the tabs `index.d.ts`, the Kotlin and Swift binding
    tests, and the CLI verbs (`knock`, `self-room`, `room-access`, `join-via-link`,
    `device-rename`, …) follow the API.
  - Docs: `docs/library.md`, `docs/bindings.md`, `docs/cli.md`,
    `docs/api-mapping-matrix-rust-sdk.md`, `docs/ARCHITECTURE.md` where the store
    contract changes, and `CHANGELOG.md` **BREAKING**.
  - Released with Step 162 as 0.6.0.

### Phase 1 (per sub-step)

- The names: matrix-rust-sdk's term where it has one, otherwise the plainest
  English.
- Typed room access: how Wire's `access` and `access_role` combinations map to a
  join rule and an allowance without losing a state Wire can hold.
- Server settings: which feature configs get a typed accessor now, and whether a
  raw map stays public at all.
- Whether the store contract's renames come with a migration of persisted names
  (expected: none, the columns stay).

## Out of scope

- Names inside `wire/`: Wire's, by rule.
- The SQL schema's column names (not public).
- SSO, typing and the email change: Step 162.

## TDD sketch

- Each sub-step: the existing tests renamed with the API, plus a parity test that
  the old names are gone from the FFI and the `.d.ts`.
- Typed room access round-trips every access/access_role combination the backend
  accepts, through `wire/`.
- A grep gate (a test) that the public surface carries none of the words the ADR
  lists: in the UniFFI exports, the wasm exports and the `.d.ts`.
- Smoke on the reference machine after 163b–e: the regular smoke, plus a ping, a
  disappearing message, room access and a join link.
