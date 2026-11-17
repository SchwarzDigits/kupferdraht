# Step 163 — The public API without Wire's vocabulary

**Status: commissioned 2026-11-17** (Volker), from an audit of the public API the same
day. ADR-0009 and its rule say Wire's names never reach the public API: matrix-rust-sdk
concepts outside `wire/`, Wire field names only inside it. The audit found about
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
- **163b — message kinds.**
  - `Knock` becomes a ping or nudge (`send_ping`, `TimelineItemKind::Ping{…}`), and
    `hot_knock` gets a plain flag name.
  - Ephemeral becomes disappearing: `send_disappearing_text`,
    `disappearing_ttl_ms`.
  - `Asset` becomes file or attachment: `MessageBody::File`, `Timeline::file()`,
    `FileSource`, `downloadFile`, `kind: "file"`.
  - Wire's locator and key fields leave the public records behind an opaque
    handle, as the browser already does.
- **163c — rooms.**
  - Room access is typed: a join rule (invite only, by link, …) and who may join
    (team members, guests, services). Wire's strings are mapped in `wire/`.
  - `join_via_link(url, password)` takes the link that `create_join_link` returns.
  - `self_room` gets a neutral name (for example a personal room).
- **163d — identity, devices, session.**
  - `QualifiedUserId` becomes `UserId`.
  - `DeviceInfo{kind, display_name, model, last_seen_ms, registered_at_ms}`,
    `initial_device_display_name`, `rename_device(id, display_name)`.
  - `cookie_label` becomes `session_label`, `handle` becomes `username`, `is_self`
    becomes `is_own`.
  - Avatar keys become an avatar source.
  - `thumbprint` becomes `fingerprint`, `backend_removal_key_ecdsa_p256` becomes
    `external_sender_key`.
- **163e — server settings and the store contract.**
  - Feature configs become typed settings, with the raw map internal or
    neutrally named.
  - User properties become account data, `ReceiptMode` a read-receipt setting.
  - The store's records and methods follow, so the Rust names change and the SQL
    columns stay.
  - The JSDoc and the error texts are reworded.
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
