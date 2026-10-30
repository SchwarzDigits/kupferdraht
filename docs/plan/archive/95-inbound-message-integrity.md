# Step 95 — Inbound message integrity: MLS-authenticated sender, sender-bound edit/delete, no panics on foreign data

**Status: MERGED 2026-09-28** — commissioned 2026-09-25 (Markus,
production-readiness review of 2026-09-25 — blockers 1–3; 95d added
2026-09-26). `395fbdf` (ADR-0027 + ADR-0028 + rules), `d770f09` (95a),
`71b5ad7` (95b), `d3757e8` (95d), `5f7eef4` (95c); review fixes after
Volker's review `9bf6823` + `056796f` (95a), `7daee55` (95b), `4128176`
+ `d009220` (95c). Merged to `main` as PRs #55 (`b751c43`), #56
(`f8b5dca`) and #58 (`c1dd640`, carrying #57); CI green. Clause (b) of
the ADR-0028 delete rule was narrowed afterwards from observation
(observations §91) in PR #62. Smoke: 95a, 95b, 95d, 95c live against
schwarz-dev-wire on 2026-09-26; the clause-(b) webapp observation
2026-09-28. Scope shipped: MLS-authenticated sender attribution
(ADR-0027), sender-bound edit/delete with a sending-side guard
(ADR-0028), room devices + MLS thumbprints + a device-change signal,
no panics on foreign data (132 runtime-data assertions turned into
typed errors). Retrospective: `docs/LEARNINGS.md` §5.36; residuals:
`open.md` §70, the review backlog §71. Archived with this header.

Sub-steps 95a, 95b, 95d, 95c in that order; one branch + one commit
each (`step/95a-…`, `step/95b-…`, `step/95d-…`, `step/95c-…`). 95b and
95d build on 95a; 95c is independent. ADR-0027 (sender authentication,
D1 + D2) and ADR-0028 (edit/delete authorization, D3) accepted
2026-09-26 (Markus), written before 95a.

## Problem (verified in code 2026-09-25)

1. **The sender of an inbound message is taken from the unauthenticated
   Wire envelope.** `sync/service.rs::extract_mls_envelope` reads
   `qualified_from` / `from_client` (falling back to `"unknown@unknown"`
   and to `subconv`); `encryption/messages.rs::process_inbound_message`
   drops openmls's authenticated `ProcessedMessage::credential()` with
   `processed.into_content()`. A compromised or malicious backend can
   re-attribute any genuine ciphertext — text, reaction, receipt, task
   (`creator_user_id`), case snapshot — to any other member. The
   ADR-0018 case gate ("MLS-authenticated sender") checks the envelope
   sender in reality. Contradicts THREAT_MODEL G1.
2. **Edits and deletes are not bound to the original sender.**
   `dispatch_inbound_edit` / `dispatch_inbound_delete` receive no sender;
   the storage UPDATEs match on message id + room only
   (`storage/sqlite/timeline_events.rs`). Any member — through an honest
   backend — can rewrite or redact anyone's message, including task
   carriers. Official Wire clients bind both to the original sender.
3. **Foreign data reaches `assert!`s.** An inbound `TaskPayload` with an
   empty `task_id` (prost does not enforce proto2 `required`) panics in
   `storage/sqlite/tasks.rs::upsert_task`; a panicking spawned sync loop
   dies silently (no `on_error`, no `on_session_expired`); on wasm the
   worker traps. Same class: `encryption/welcome.rs` (under the MLS
   mutex — poisons it), `sync/push/mod.rs` (persisted device id),
   `client/resume.rs` (blanked refresh token after `SessionExpired`,
   debug builds), `UserId::new` on event JSON, the peer-supplied
   ephemeral TTL (`received_at_ms + ttl` overflow). Violates the
   CLAUDE.md Assertions rule (runtime data is never asserted).

## Decisions (Markus, 2026-09-25)

- **D1 — The MLS credential is the authoritative sender.** The sender
  (user id + device id) of every inbound application message is parsed
  from the leaf credential openmls authenticated. The envelope's
  `qualified_from` / `from_client` is a cross-check only: on mismatch
  the message is dispatched under the MLS sender and a `warn!` records
  the mismatch (no drop — the message itself is authentic).
- **D2 — BasicCredentials are authenticated; X.509 senders keep the
  envelope attribution, for now.** A sender whose leaf credential is not
  a Wire `BasicCredential` — an e2ei X.509 credential — is attributed to
  the envelope's `qualified_from` / `from_client` exactly as before
  (no regression for mixed e2ei teams; the credential type is fixed by
  the signing leaf, so the backend cannot route a Basic-signed message
  into this path) and logged at `debug!`. A Basic credential that does
  not parse as W-4 (`{uuid}:{client}@{domain}`) is malformed and dropped
  with a `warn!`. X.509 identity parsing is a follow-up step, started
  once a real certificate is available (the SAN format is not publicly
  specified; the own instance has e2ei disabled); a public
  `sender_verified` flag is considered there, not now. The parser
  module is shaped so the X.509 arm is an addition.
- **D3 — Edit/delete are bound to the original sender's user (Wire
  parity).** Both require the target in the same room. An **edit**
  applies only when its (MLS) sender's user id equals the target row's
  `sender_user_id` (any device of that user). A **delete** applies when
  (a) its sender's user is the target's sender, or (b) its sender is
  the **self user** and the target is **ephemeral** (another own
  device syncing an expiry — narrowed 2026-09-28 after the webapp was
  observed to ignore a self-sent delete of an ordinary message,
  observations §91), or (c) the
  target is the **self user's own ephemeral** message (from any member:
  Wire enforces ephemeral expiry by the *receiver* sending the delete,
  observations §74; Kupferdraht's Step-89 receivers do the same). A third
  party's delete of someone else's ephemeral message is rejected —
  every receiver enforces its own timer. No admin override. A rejected
  edit/delete is dropped with a `warn!`.
- **D4 — 95c scope:** every assertion on runtime data becomes a typed
  error; Kupferdraht's own payloads are validated at the decode boundary; the
  spawned sync loops report a panic as a terminal error instead of dying
  silently; the MLS mutex recovers from poisoning (openmls reloads group
  state from storage per operation, so no half-state lives in memory).
  The remaining ~30 cache-lock `.expect()` sites are backlog (open.md).
- **D5 — A read-only device-verification surface (95d), 2026-09-26.**
  ADR-0027 authenticates *which leaf* sent a message; the backend stays
  the key directory, so an injected (ghost) device must at least become
  *visible* and *comparable*. 95d exposes, per room, every leaf's user,
  device, credential kind and MLS thumbprint, the own device's
  thumbprint, and an event when a room's device set changes. The
  thumbprint is the RFC 7638 JWK thumbprint of the leaf's signature
  public key (SHA-256 for ciphersuite `0x0002`, base64url without
  padding) — the value official Wire apps compute for Basic and X.509
  leaves alike and show as "MLS Thumbprint" in the device details (own
  devices and other users' device cards, rendered in 2-character
  groups), so users can compare across clients. The devices-changed
  event has **no Wire counterpart**: Wire shows member-level system
  messages (backend-driven) and, with e2ei only, a "degraded"
  conversation state — without e2ei a new device in a group is not
  surfaced at all. The event is a local observation of the MLS tree,
  invisible to other clients (no protocol or interop effect), and the
  only signal against an injected device on a non-e2ei backend. **No local
  trust store** (mark verified, room shield, key-change warning) in
  Step 95: Wire has no manual MLS device verification (its MLS
  verification is e2ei-only), so that is a Kupferdraht-specific feature with
  its own decisions — a later step with its own ADR.

## Sub-steps

### 95a — MLS-authenticated sender

- `encryption/leaf_identity.rs` (new): the one Wire-identity module —
  build (`wire_identity`, moved) and parse (`LeafIdentity { user_id,
  device_id }` from an openmls `Credential`; the parts kept verbatim — no
  case folding or zero padding, compared exactly as the API spells them;
  non-Basic → typed `Unsupported`). Replaces the two parsers
  in `group_state.rs` and `group_kick.rs`.
- `process_inbound_message` captures the credential before
  `into_content()`; `DecodedMessage::Application` carries the parsed
  sender.
- `sync/service.rs`: dispatch uses the MLS sender for every handler
  (text, reaction, receipt, knock, location, asset, ephemeral, task,
  case, marker); envelope mismatch → `warn!`; X.509 credential →
  envelope attribution (D2); malformed Basic credential → drop +
  `warn!`. The ADR-0018 case gate becomes what the ADR says.
- Docs: THREAT_MODEL G1 note + the "server not trusted with content"
  correction (key directory under BasicCredentials, ADR-0027);
  `docs/wire/mls-deviations.md` W-4 (one parse path); ARCHITECTURE §5.3;
  `docs/library.md` (sender semantics). No public signature change —
  `CHANGELOG` `Changed` (the sender is now authenticated).

TDD (`tests/step_95a_mls_authenticated_sender.rs`):
- envelope claims Alice, MLS leaf is Bob → the row, the `EventHandler`
  message and the timeline item carry Bob;
- envelope without `qualified_from` → the MLS sender, never
  `unknown@unknown`;
- envelope/MLS mismatch emits a `warn!` and still dispatches;
- a case snapshot from a non-admin leaf under an admin's envelope does
  not fold (ADR-0018 gate on the MLS sender);
- a task's `creator_user_id` is the MLS sender;
- a message signed by an X.509 leaf is dispatched under the envelope
  sender (D2 — status quo, no drop);
- a message whose Basic credential is not W-4 is dropped;
- unit: `LeafIdentity` parses W-4 verbatim, rejects malformed Basic
  credentials, reports X.509 as unsupported (not malformed).

### 95b — Sender-bound edit / delete

- `dispatch_inbound_edit` / `dispatch_inbound_delete` take the MLS
  sender; before applying they resolve the target with
  `Store::load_timeline_event` and enforce D3 (same room; edit: same
  user; delete: same user, self user, or own ephemeral target). The
  target lookup is room-scoped — new `Store::load_timeline_event_in_room`
  (default body, non-breaking): rows are keyed `(message_id, room_id)`.
- **Sending side (added 2026-09-26, Markus, after the live smoke showed
  the attacker's own view diverging):** `Timeline::edit` /
  `Timeline::redact` refuse another user's message with `InvalidInput`
  before uploading (own = any device of the self user; an id unknown
  locally goes out unchanged; the receiver-side ephemeral expiry is a
  separate internal path). ADR-0028 revision-history note.
- ADR-0028 covers D3; `docs/library.md` edit/delete sections;
  ADR-0014 carrier-interaction note (a task carrier is deletable by its
  sender only).

TDD (`tests/step_95b_sender_bound_edit_delete.rs`):
- a member's edit / delete of another member's message → row unchanged,
  no `UpdateAt` diff;
- the original sender's edit / delete from a second device of the same
  user → applied;
- a member's delete of **our own** ephemeral message → applied
  (Step 89 receiver-side expiry, sender side);
- a member's delete of a **third party's** ephemeral message →
  rejected (each receiver runs its own timer);
- a delete from another device of the **self user** of someone else's
  message → applied (own-device sync of the receiver-side delete);
- an edit / delete naming a message of another room → ignored;
- a member's delete of another member's task carrier → task fold
  unchanged;
- sending side: `Timeline::edit` / `redact` of another user's message →
  `InvalidInput`, nothing uploaded, row unchanged; own messages (incl.
  another own device's) stay editable / deletable.

### 95d — Read-only device verification surface

**Built 2026-09-26** on `step/95d-device-verification-surface`. Phase-1
decisions (Markus): a separate client-wide listener
(`Client::subscribe_device_changes`, the `Encryption::devices_stream`
shape — additive, no break for foreign `EventHandler`s), a flat list
per device, and diff events against a **persisted** baseline (V29
`room_devices`) rather than Wire's bare epoch signal. Wire, checked:
the device list with thumbprint comes from the MLS group's leaves
(same approach); changes are observed via an internal epoch-change
observer used only for e2ei verification and calling — no user-facing
device signal without e2ei. **Live-smoke finding (fixed in 95d):** a
room whose first epoch change after the upgrade was a new device
recorded that device as its silent baseline — the change was lost.
`sync_once` now records a baseline for every room lacking one before it
drains (`ensure_device_baselines`, test
`a_room_without_a_baseline_gets_one_before_the_next_drain`). Smoked
2026-09-26: a second device of user 2 removed (`logout --purge-device`)
and added (fresh login + `rooms-join-all`) — user 1's `sync` printed the
`-` / `+` change in both shared rooms, with the thumbprint the device
reports for itself.

- Thumbprint: RFC 7638 over the P-256 signature public key of a leaf
  (`{"crv":"P-256","kty":"EC","x":…,"y":…}`, SHA-256, base64url) — an
  encoding + standard hash, no new primitive, no new dependency
  (`sha2`, `base64`). The API returns the raw string; the CLI prints it
  in 2-character groups as the Wire apps do, and the docs say "compare
  with the MLS Thumbprint in the Wire app's device details".
- Surface (names settled in phase 1 against matrix-rust-sdk's
  `Encryption::get_user_devices` / `Device`, ADR-0009): the own
  device's thumbprint; per room, the device list (user id, device id,
  credential kind Basic/X.509, thumbprint — an X.509 leaf is listed
  with its kind and thumbprint, identity unparsed until the X.509
  step); an event when a room's device set changes (added / removed),
  detected around commit merge, Welcome absorption and external join.
- Phase-1 decisions: event delivery shape (a new `EventHandler` method
  vs. a separate listener — a UniFFI callback interface gains no
  default methods, so a new method breaks foreign implementors);
  whether the list folds devices per user.
- Mirrors: FFI, wasm, `bindings/web/kupferdraht-tabs/` proxy + `.d.ts`
  (ADR-0026 parity pin). CLI: `room-devices --id <room>` and the own
  thumbprint (e.g. in `status` or `device-thumbprint`).
- Docs: `docs/library.md`, `docs/bindings.md`, `docs/cli.md`,
  `docs/api-mapping-matrix-rust-sdk.md`, THREAT_MODEL (ghost devices
  visible, not prevented).

TDD (`tests/step_95d_device_verification_surface.rs` + unit tests):
- thumbprint known-answer vector for a fixed P-256 key (independently
  computed);
- the room device list carries every leaf with user, device, kind and
  thumbprint; the own entry's thumbprint equals the own-device
  thumbprint;
- a peer's Add commit → one devices-changed event naming the added
  device; a Remove → the removed device;
- an X.509 leaf is listed (kind X.509, thumbprint present), not an
  error (today `group_leaves` fails on it).

### 95c — No panics on foreign data

- Inventory every `assert!` / `debug_assert!` / `expect` / unchecked
  arithmetic / indexing in library code by operand provenance (code
  invariant vs. runtime data); runtime-data sites become typed errors
  (known: `storage/sqlite/tasks.rs`, `encryption/welcome.rs`,
  `sync/push/mod.rs`, `client/resume.rs`, `UserId::new` on event JSON,
  `wire/http/assets.rs` `asset_domain`, the ephemeral TTL add).
- `validate_*` at the decode boundary for `TaskPayload`, `CasePayload`,
  `TaskListMarker` (required fields, length caps); invalid → dropped
  like a malformed envelope.
- Sync-loop supervision (FFI `start_sync`, wasm `live_sync`, the push
  loop): a panic surfaces as a terminal error to the listener.
- MLS mutex: poison recovery.

**As built (2026-09-26).** The inventory found 132 of about 280 sites
guarding runtime data — far more than the known list (see
`docs/observations.md` §90). They trace to about 30 causes, each fixed
where the data enters:

- **Peer payloads.** Invalid tasks are treated as absent (the `Text`
  fallback shows); cases without `case_ref` and mentions without a
  usable user id are dropped. Inbound TTLs are clamped to the send-side
  range. `set_case` at revision `u64::MAX` is refused. The task fold
  skips rows with an empty `task_id` that pre-95c databases can hold.
- **Backend data.** It is checked in `wire/`: token bodies, the
  registered client id, `/self`, the removal key, push frames (id,
  `delivery_tag`), an empty `mls-message-add` / `GroupInfo`, and URL
  path segments (RFC 3986 unreserved characters only). Conversation
  metadata is sanitised: no id → skipped, empty `group_id` / team →
  none, and the list is fetched in 1000-id chunks.
- **Stored state.** Resume turns a cleared refresh token into
  `SessionExpired`, an empty client or device id into `Storage`, and an
  empty MLS key into `InvalidInput`. An empty access token counts as
  stale. A legacy `PURE_CIPHERTEXT` group is `InvalidInput`.
- **App input.** `client/input.rs::require_non_empty` runs at every
  public entry, and `UserId::parse` checks invitees. The new
  `UserId::from_parts` builds ids from runtime data. `FileKeyStore`
  refuses path-unsafe ids.
- **Containment.** `ffi/sync.rs::report_runner_panic` wraps both
  runners and reports a panic via `on_error`, then stops. The MLS mutex
  recovers from poison (`encryption/mls_lock.rs`).

Deviations from the plan above:

- **wasm cannot be supervised** — a panic traps the module; this is
  documented instead (`docs/bindings.md`).
- **Payload length caps are deferred** to `open.md`.
- **`TaskListMarker`** has no field that could be invalid (one `bool`).
- **Downstream assertions stay.** The `debug_assert!`s on token and
  device id below the entry checks are kept as code invariants: once
  the entry points validate, runtime data can no longer reach them.

TDD: `tests/step_95c_hostile_inbound.rs` (10), `step_95c_hostile_backend.rs` (9),
`step_95c_consumer_input.rs` (4), plus 11 unit tests.

## Out of scope (follow-ups)

- **kupferdraht-tui must surface the new `InvalidInput`** from
  `Timeline::edit` / `Timeline::redact` on another user's message (95b):
  its backend (`src/backend/mod.rs`) maps both results with
  `.map(|_| ())`, so the refusal would be a silent no-op for the user.
  Tracked for the kupferdraht-tui repo (review note on PR #56, 2026-09-28).
- **X.509 sender identity** (D2) — own step once a real e2ei
  certificate is available; until then X.509-signed messages keep the
  unauthenticated envelope attribution.
- **Leaf identity continuity on Update / UpdatePath** (RFC 9420 §5.3.1
  successor check) — moved to the MLS state-hardening follow-up: a
  rejected commit is undone today by the `WrongEpoch` → `recover_room`
  → external-rejoin heal, so the check only works together with
  narrower heal triggers (review findings F3/F4: epoch-gated heal,
  sender/type-filtered proposal auto-commit).
- Key-package / Welcome identity binding (the backend is the key
  directory under BasicCredentials) — a THREAT_MODEL correction, not
  code; 95d makes injected devices visible and comparable.
- **Local device trust store** (mark a device verified, room
  verification state, warning on a new or re-keyed device) — a later
  step with its own ADR (D5; Kupferdraht-specific, no Wire MLS parity).
- Typing indicators (`conversation.typing`) are not MLS messages and
  stay unauthenticated by design — documented, not changed.
