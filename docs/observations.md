# Kupferdraht — Observations

*Convention (Step 83): a struck-through heading marks an observation superseded by a later finding or shipped fix — the one-line note under it says by what. The text below it is never edited; this file is a findings log. The entries of April 2026 were written in German and translated to English on 2026-10-08, content unchanged.*

Empirical Wire/MLS observations from live testing, cited as evidence by the ADRs, api-notes, and PLAN. Migrated from the former findings.md; the original entry numbers (§N) are preserved so existing "findings §N" citations map one-to-one to "observations §N".

## Step 3e: mechanically validated (2026-04-22)

The self-claim test against a live Wire backend confirmed the complete
key-package claim path:

- Request: `POST /v5/mls/key-packages/claim/{domain}/{uuid}?ciphersuite=1&skip_own=<own-client>`
- Response: `200 OK` with a real `key_package` for our own user.

So URL construction, auth headers, the ciphersuite parameter (decimal,
not hex), `skip_own` and response parsing are implemented correctly in
Kupferdraht. The recurring `{"key_packages":[]}` for other Wire accounts
is real backend state (empty pools, because the target client has not
triggered a refill for a while), not a Kupferdraht bug.

The commit-bundle upload (`POST /v5/mls/commit-bundles`) is complete in
the code (`http::upload_commit_bundle`) but, for want of a non-empty
pool, could not yet be tested end to end against the live backend. We
close Step 3e as mechanically validated nonetheless and catch up on the
end-to-end verification as soon as a cooperating test account with a
filled pool is available.

---

## 7) Observation: `skip_own` has no effect on a self-claim

On a self-claim (`claim` with `user = self`, `skip_own = own_client_id`)
Wire returns a key package of exactly that client despite `skip_own`:

```
request:  ...?ciphersuite=1&skip_own=<client-id>
response: {"key_packages":[{"client":"<client-id>", ...}]}
```

Possible readings, none confirmed:

1. `skip_own` is only evaluated on claims against *other* users.
2. `skip_own` is ignored when the target user has only a single client
   (Wire would otherwise return an empty result).
3. A server-side bug or undocumented behaviour.

**Assessment:** low. Irrelevant to the Step 3e mechanics, because the
regular group create claims against *other* users. To be revisited at
the first bug around double adds.

---

## ~~14) API version backlog (2026-04-22)~~

*Closed 2026-09-24 — superseded by the v14-only pin (ADR-0008, Step 6 v14 upgrade). Text kept unchanged as the findings log.*

Kupferdraht's `version_info::CLIENT_SUPPORTED = &[5]`: we are pinned to
Wire API **v5**. The state of the world:

- Wire's Swagger (`reference/wire.swagger.json`) describes **v14**.
- Kalium supports v0–v15.
- Deployed wire-servers accept everything between v0 and v14+,
  backwards-compatibly.

**Why v5 keeps working:** Wire keeps older versions active. Our login,
`/clients`, `/mls/*` and all sync endpoints run green; nothing to do
as long as v5 is alive.

**What changed between v5 and v14 that matters:**

- **v0–v8 (classic):** event stream over the `/await` WebSocket plus
  `/notifications` REST backfill. JSON frames, no ack protocol.
- **v9** *(the big rework)*: consumable notifications — the client
  has to confirm every event explicitly via `EventAcknowledgeRequest`;
  only then does the server remove it from the queue. New `/events`
  endpoint, considerably more state management. Kalium
  `NotificationApiV9.kt`.
- **v10–v14:** refinements on the v9 basis.

**Decision (2026-04-22):** keep v5 for Step 6. Reasons:

1. The `/await` + `/notifications` path is simpler and fits the PoC
   philosophy of this phase.
2. Upgrading now would mean re-validating Steps 1–5 HTTP call by HTTP
   call; that is a step of its own, not a side effect.
3. The v5 retirement risk is real but not acute; Wire has kept
   versions active for one to two years in the past.

**Open as a future step (not yet planned):** a **version-upgrade
step**. Raise `CLIENT_SUPPORTED` version by version, validate each one
end to end (login → conversations → group CRUD → send → receive), with
Kalium's V* classes as the diff source. Logically before or alongside
Step 4b. No new ADR needed: `select_api_version` already implements
"max(intersect(client, server))"; we only change the client allow-list.

**Update (2026-04-22, after §15):** the urgency has shifted. The
version upgrade is now a **hard blocker for Step 6**, no longer
optional. Details in §15.

---

## ~~15) Step 6 failed on v5 — the v14 upgrade is the next step~~

*Closed 2026-09-24 — resolved by the v14 upgrade (ADR-0008); the v5 `/await` route is retired. Text kept unchanged as the findings log.*

**Symptoms (2026-04-22 against a live Wire backend):**

1. `wss://<backend>/v5/await?client=…` → `403 Forbidden` (generic
   nginx page, 146 bytes). The **versioned** route for the legacy
   event stream no longer exists at Wire's nginx.
2. `wss://<backend>/await?client=…&access_token=…` (without the `/v5/`
   prefix, as Kalium's `NotificationApiV0` does it) → handshake
   `101 Switching Protocols` ✓.
3. The stream receives **binary frames only** (400–1000 bytes), never
   text frames. JSON deserialisation from the bytes fails.
4. While iOS writes messages in parallel: no
   `conversation.mls-message-add` events reach our CLI. After a
   tolerant parse skip plus a diagnostic hex dump, *no* frames came at
   all; the connection stays open, the upstream delivers no traffic.

**Diagnosis:** wire-server keeps the `/await` nginx route open for
older clients (the handshake succeeds), but the upstream dispatcher no
longer serves it with the classic JSON event format. v9 brought
`ConsumableNotifications` (per-event ack, new `/v{N}/events` endpoint,
see §14); v14 is the Swagger-documented state. Our v5 model is **no
longer functional** on the event-stream side, even though HTTP calls
(login, conversations, MLS crypto CRUD) are still served.

**Consequence:** the version-upgrade step (§14) turns from a
"some day" item into a **blocker before any further Step 6 work**.
Kupferdraht jumps straight to **v14**, not version by version:
validating intermediate versions costs more than jumping to the code
path Wire actively runs.

**Already committed on `step/6-event-stream` and not to be thrown
away:**

- `kupferdraht-core/src/events.rs` — WebSocket connect with
  `tokio-tungstenite`, event-frame parsing, tolerant error handling.
  The handshake logic (auth via the `access_token` query parameter plus
  Bearer header plus Z-Client, `/await` without a version prefix)
  stays as a reference for the v14 rewrite.
- `MlsClient::decrypt_message` + `process_welcome_message` in
  `kupferdraht-core/src/crypto.rs` — MLS crypto is version-independent
  (the RFC 9420 wire format does not change between Wire API
  versions) and stays usable as is.
- `Session::listen_events(observer)` + the `MessageObserver` trait +
  the `ReceivedMessage` dictionary in `kupferdraht-core/src/session.rs`
  and the UDL — the public API fits v14 too; only the internal
  event-sourcing layer changes (connect path plus ack protocol).
- CLI `listen --format pretty|json --id <filter>` in
  `kupferdraht-cli/src/commands/listen.rs` — hangs off the observer
  interface, not the wire format. Works unchanged.

**To be written anew for the v14 rework:**

- `connect_events_ws`: URL on `/v14/events`, sync-marker parameter,
  WebSocket protocol handshake for v9+.
- Event-frame parser: the v9+ `ConsumableNotifications` envelope
  instead of the current `EventResponse` structure.
- Write a per-event `EventAcknowledgeRequest` after we have processed
  the event (otherwise Wire does not clear the queue).
- `CLIENT_SUPPORTED = &[14]` (or `&[5, 14]` with version selection).
- Walk every existing HTTP endpoint once and compare its shape against
  Kalium's V6–V15 classes.

**Cost estimate:** one or two focused working days. Existing Steps 1–5
+ 2b/1 + 4a + the MLS crypto core stay valid in substance; the work is
mainly API-version shape alignment plus the Step 6 rework.

## 16) Wire login requires `label` in the JSON body for the `zuid` cookie (2026-04-23)

`POST /v{N}/login?persist=true` does **not** set a persistent `zuid`
`Set-Cookie` header unless the JSON body carries a non-empty `label`
field. The query parameter alone is insufficient — Wire's auth
service correlates the `persist=true` flag with the `label` to name
the refresh-cookie slot, and silently omits the cookie when the label
is missing.

**Failure mode:** login returns `200 OK` with a valid access token,
but no `zuid` Set-Cookie. Every subsequent `POST /v{N}/access` then
fails with `403 "Missing cookie and token"` because the refresh path
has no cookie to authenticate against (see §17).

**Fix:** `wire::http::DEFAULT_COOKIE_LABEL = "kupferdraht-cli"` is always
included in the login body. The regression-guard test lives in
`kupferdraht-core/src/wire/http.rs::tests::build_login_body_includes_default_cookie_label`.

---

## 17) `POST /v{N}/access` is cookie-authenticated only; Bearer is rejected (2026-04-23)

Contrary to every other authenticated Wire endpoint, `POST /v{N}/access`
authenticates **only** via the `zuid` refresh cookie. Sending the
access token as an `Authorization: Bearer <token>` header returns
`403 "Missing cookie and token"` — the server actively refuses the
Bearer auth on this endpoint.

This differs from the rest of the v14 surface (`/self`, `/clients`,
`/notifications`, `/conversations`, …), which all use Bearer auth.
The `/access` endpoint is the one exception because it exists to
**re-issue** access tokens from the refresh credential, so the access
token itself cannot be the auth material.

**Fix:** `HttpClient::rebind_access_token` sends `Cookie: zuid=<value>`
and no `Authorization` header. Documented in the function's doc
comment.

---

## 18) reqwest cookie store does not reliably forward Wire's `Path=/access`-scoped cookie (2026-04-23)

`HttpClient::new` enables reqwest's automatic cookie store
(`cookie_store(true)`), expecting it to forward the `zuid` cookie from
the login response to `POST /v{N}/access`. In practice the cookie is
**not** attached — the Set-Cookie's `Path=/access` scope combined with
reqwest's internal cookie-jar handling means the cookie does not travel
to subsequent `/access` requests consistently across OS/TLS stacks.

**Failure mode:** login succeeds and reqwest acknowledges the Set-Cookie
(visible in tracing), but the next `POST /v{N}/access` returns
`403 "Missing cookie and token"` — the cookie jar did not attach the
cookie to the outgoing request.

**Fix:** extract the `zuid` value from the login response's Set-Cookie
header manually (`wire::http::extract_zuid`), thread it through
`LoginOutcome::refresh_token`, and set `Cookie: zuid=<value>` explicitly
on the `/access` request. The reqwest cookie store stays enabled for
other endpoints where it works, but `/access` is handled explicitly.

**Regression guards:** `extract_zuid_parses_from_set_cookie`,
`extract_zuid_returns_none_when_no_zuid_cookie`, and
`extract_zuid_finds_value_across_multiple_set_cookie_headers` in
`kupferdraht-core/src/wire/http.rs`.

---

## 19) Wire requires the MLS public key to be registered via `PUT /v14/clients/{id}` before `POST /v14/mls/key-packages/self/{id}` (2026-04-23)

The Wire-production live smoke of Step 1c surfaced a 400 on the
KeyPackage upload with the label `mls-protocol-error` and the message
`"No key associated to the given identity and signature scheme"`.

Diagnosis: Wire validates each uploaded `KeyPackage` against the
`(identity, signature_scheme)` pair it has on record for the client.
`POST /v14/clients` (Step 1b) does accept an `mls_public_keys` block
inline — but the flow Kupferdraht uses today generates the signature
keypair *after* registration (because the Wire identity is built from
the returned `client_id`). A separate step is therefore required.

**Fix:** immediately after `register_and_rebind` but before the
KeyPackage upload, `PUT /v14/clients/{client_id}` with a body of
`{"mls_public_keys": {"ecdsa_secp256r1_sha256": "<base64>"}}`. The
endpoint returns `200 OK` and subsequent uploads are accepted.

Wire response shape:

```
PUT /v14/clients/<client_id>
Authorization: Bearer <rebound-access-token>
{ "mls_public_keys": { "ecdsa_secp256r1_sha256": "<b64>" } }
→ 200 OK  (empty body)
```

Regression guard: every integration test that exercises login flow
mounts an additional `PUT /v14/clients/new-client-id` mock. If a
future refactor drops the registration step, the missing mock causes
the login to 404 and the tests go red in a way that points at the
failing endpoint.

**Kupferdraht wiring:** `HttpClient::update_client_mls_public_key` in
`kupferdraht-core/src/wire/http.rs`; called from
`LoginPipeline::upload_initial_key_packages` right after
`MlsClient::new_fresh_with_storage`.

---

## 20) Wire deviates from RFC 9420 in nine concrete places (2026-04-25)

Live-validation of Step 3 (room-join via external commit) against
Wire production surfaced two new MLS-protocol deviations and
re-confirmed seven previously-documented ones, motivating a
consolidated reference document.

**New deviations discovered during this session:**

* `GET /v14/conversations/{d}/{id}/groupinfo` returns a raw
  TLS-serialized `VerifiableGroupInfo` body, not an
  `MlsMessage(GroupInfo)` envelope. Live observation: leading byte
  was `0x20`, not the `0x00 0x01 0x00 0x04` MlsMessage prefix.
  `MlsMessageIn::tls_deserialize` failed with `UnknownValue(32)`.
  Fix: try MlsMessage parsing first, fall back to
  `VerifiableGroupInfo::tls_deserialize`.
* `ExternalCommitBuilder` rejected an existing peer leaf with
  `LeafNodeValidation::Lifetime(NotCurrent)`. The offending lifetime
  was `{ not_before: 1769693151, not_after: 1776954351 }` (~84 days)
  — Wire's clients pick a wider acceptance window than openmls's
  default. RFC 9420 §7.5 explicitly leaves the window to the
  application. Fix: `ExternalCommitBuilder::skip_lifetime_validation()`
  for the pre-existing tree (the local leaf still goes through full
  validation).

**Previously documented deviations re-confirmed:**

W-2 commit-bundle triple framing (LEARNINGS §1.5), W-4 identity
packing `{uuid}:{client_id}@{domain}` (LEARNINGS §1.3), W-5 mandatory
`external_senders` extension (ADR-0004), W-6 P-256 ciphersuite
pinning (LEARNINGS §1.2), W-7 `[Basic, X509]` capabilities (LEARNINGS
§1.3), W-8 nginx `Z-Client` injection (LEARNINGS §1.4), W-9 PUT
`/v14/clients/{id}` MLS-pubkey pre-registration (this finding's §19).

**Reference document:**
[`docs/wire/mls-deviations.md`](docs/wire/mls-deviations.md) — read
before adding any MLS-touching code. Cross-referenced from CLAUDE.md's
documentation map. Future Step 4 (send) and Step 5 (receive) will
likely add W-10 / W-11 entries; the maintenance protocol at the
bottom of the document keeps the format consistent.

---

## 21) Wire rotates the `zuid` refresh cookie on every `/access` (2026-04-26)

Step 5a's first live-smoke against Wire production failed
immediately after `login → sync` with:

```
kupferdraht-cli error: backend rejected the request:
  rebind_access_token: 403 Forbidden
  body={"code":403,"label":"invalid-credentials","message":"Invalid zauth token"}
```

`login` had succeeded one second earlier — the failure path was
`sync → ClientBuilder::resume → rebind_access_token`. Diagnosis:
Wire **rotates** the `zuid` refresh cookie on every successful
`/v14/access` call. The login pipeline already does one `/access`
rebind internally to bind the access token to the freshly-registered
client_id (LEARNINGS §1.4 / observations.md §16). That call rotates the
cookie on Wire's side. The pre-rotation value is what `login` had
just persisted; sending it back through a second `/access` returned
403 invalid-credentials.

**Fix:** `HttpClient::rebind_access_token` now returns a
`RebindOutcome { access_token, refresh_token }` carrying the new
cookie extracted from the response's `Set-Cookie` header (falls back
to the input value when Wire didn't rotate). Both call sites —
`LoginPipeline::register_and_rebind` and
`ClientBuilder::resume` — capture and persist the rotated value. The
resume path immediately calls `Store::save_session` with the new
zuid before continuing, so a crash mid-resume doesn't leave the
on-disk session pinned to the just-invalidated cookie.

**Cross-reference:** related Wire-auth landmines — observations.md §16, §18, §19.

---

## ~~22) Wire's v9+ event stream is opt-in via the `consumable-notifications` client capability (2026-04-26)~~

*Closed 2026-09-24 — superseded by §68 (ConsumableNotifications hard-disabled upstream) and ADR-0019 (transport auto-negotiation). Text kept unchanged as the findings log.*

Step 5a's first end-to-end live-smoke against Wire production
showed the WebSocket connecting cleanly (no error printed, no
401), but no frames arriving — neither for the round-trip-test
message Kupferdraht itself had just sent, nor periodic sync markers.
30-minute idle confirmed Wire was simply not delivering anything.

Diagnosis: Wire's `/v14/events` ConsumableNotifications stream
requires the device to have declared the `consumable-notifications`
capability. Without it, Wire keeps the device on the legacy
`/v14/await` model and the new endpoint is silent. Kalium's
`UpdateSelfClientCapabilityToConsumableNotificationsUseCase`
(`logic/.../client/UpdateSelfClientCapabilityToConsumableNotificationsUseCase.kt`)
PUTs the capability for every v9+ client.

**Capability shape** (Kalium's `ClientCapabilityDTO` serialization):

```json
PUT /v14/clients/{client_id}
Authorization: Bearer <token>
Content-Type: application/json

{ "capabilities": ["consumable-notifications", "legalhold-implicit-consent"] }
```

The endpoint is the same `/v14/clients/{id}` that already accepts
the `mls_public_keys` body (observations.md §19); Wire treats the body as a
patch — only the keys present are updated.

**Kupferdraht fix:**

* New `HttpClient::update_client_capabilities` issues the PUT.
* `LoginPipeline::upload_initial_key_packages` calls it
  immediately after `update_client_mls_public_key`, so every fresh
  device starts with both capabilities declared.
* The PUT is idempotent — re-issuing the same set is a server-side
  no-op, so future remediation paths (e.g. running `sync-stream`
  on a device registered before this fix) can safely re-PUT
  without burning a slot or invalidating sessions.

---

## 26) `add_members` with `force_self_update=true` trips `Client identity does not match credential identity` against an epoch-0 group (2026-04-28)

Step 6b's first attempt at `Room::invite` called openmls's
`MlsGroup::add_members`, which defaults to `force_self_update=true`
— the commit re-encodes the inviter's leaf with new keys and
re-signs it with the existing credential. Wire's commit-bundle
parser rejects:

```
upload_commit_bundle 400 Bad Request
{"code":400,"label":"mls-protocol-error",
 "message":"Tried to add invalid LeafNode: Client identity does not
            match credential identity"}
```

This is the same error class that blocked Step 6a's self-update
bootstrap experiment (CHANGELOG.md §25). Wire validates the
path-update leaf's identity against its server-side records for
that client and rejects whatever combination openmls produces under
`force_self_update`. The exact validation rule on Wire's side is
opaque from the client.

**Resolution:** switched to
[`MlsGroup::add_members_without_update`](https://docs.rs/openmls/0.8/openmls/group/struct.MlsGroup.html#method.add_members_without_update).
RFC 9420 §17.4 permits Add proposals without a path; the new
members' KeyPackages already carry their own signed leaves, so
no path is needed for the commit to bind every member to the new
epoch. The committed code in
`encryption/group_invite.rs::add_members_commit` uses
`add_members_without_update` exclusively. This unblocked the next
error class — see §27.

---

## 27) `mls-client-mismatch` on the first `Room::invite`: Wire requires every device of every member-user to be a leaf (2026-04-28)

After §26 was resolved, the next live-smoke attempt at
`Room::invite` hit:

```
upload_commit_bundle 409 Conflict
{"code":409,"label":"mls-client-mismatch",
 "message":"A proposal of type Add or Remove does not apply to the
            full list of clients for a user"}
```

The room had been created via `Client::create_room` with one leaf
(this device's MLS identity). Wire's user-registry, however, had
multiple Wire clients on file for the same user — older
`kupferdraht-cli login` runs each registered a fresh client_id, plus
the production iOS / desktop apps registered earlier. Wire enforces
a "every device of every member-user is a leaf" invariant at
commit-validation time. With only one of the user's leaves in the group
but four+ of the user's clients in Wire's records, the Add commit for an
external invitee (the second account) was rejected because the user's
device-coverage was incomplete.

**Resolution (committed in `encryption/group_invite.rs` and
`client/builder.rs`):**

1. **Self-bootstrap claim** — `Client::invite_members` now claims
   the caller's own KeyPackages first, with
   `skip_own=Some(self.device_id)`. Wire returns one KP per other
   client the user owns. These are appended to the Add list before
   the invitee's KPs.
2. **Duplicate-leaf filter** — `MlsClient::add_members_commit`
   builds a `HashSet<Vec<u8>>` of the group's existing leaf
   signature keys and drops any input KP whose leaf signature key
   is already in the tree. This keeps subsequent invites
   idempotent: once the first invite seeds the inviter's other devices,
   later invites re-claim self KPs but silently skip the
   already-present leaves. `add_members_without_update` would
   otherwise reject duplicates with `DuplicateSignatureKey`.
3. **`skip_own` policy** — only set when claiming for self
   (the original endpoint contract still holds: omitting it
   for self gets back an empty pool defensively); set to `None`
   when claiming for another user (their pool can't contain the
   caller's `client_id` anyway).

Live-validated 2026-04-28: `Kupferdraht 6b smoke 3` advanced from
epoch 0 to epoch 1 on the first invite, member count 1 → 2,
and `kupferdraht-cli send` then accepted application messages —
closes CHANGELOG.md §25.

---

## 28) `400 bad-request: "Missing commit"` is a wire-format-policy issue, not a path/epoch one (2026-04-29)

Step 6c's first live-smoke attempt — invite a third user into an
already-running 2-member room — failed against Wire production
with:

```
upload_commit_bundle 400 Bad Request
{"code":400,"label":"bad-request","message":"Missing commit"}
```

The previous-cycle hypothesis (recorded in `docs/wire/api-notes.md`) was
that Wire requires every commit to carry a path-update, and that
the §26 "credential identity does not match" error only fires at
epoch 0. Both halves turned out to be wrong:

1. A re-test against a freshly-created room at epoch 1 (so a
   Kupferdraht-managed group, MIXED_PLAINTEXT, no peer-state ambiguity)
   with `add_members` (`force_self_update=true`) was rejected with
   the **same identity-mismatch error from §26** — confirming
   that error is *not* epoch-0-specific. Wire forbids
   path-update leaves on Add commits unconditionally; the
   correct call is always `add_members_without_update`.
2. The actual root cause of "Missing commit" against the original
   joined-via-external-commit room ("Kupferdraht Test 2", epoch 13)
   was the wire-format-policy on that group: openmls's
   `MlsGroupJoinConfig::default()` is
   `PURE_CIPHERTEXT_WIRE_FORMAT_POLICY`, so outgoing handshake
   messages shipped as `PrivateMessage`. Wire's
   `/mls/commit-bundles` parser cannot inspect encrypted commits
   — it has no group secrets — and folds the unparseable body
   into `bad-request: "Missing commit"`.

**Resolution (committed in `encryption/external_commit.rs` and
`encryption/group_invite.rs`):**

1. `run_external_commit` now sets
   `wire_format_policy(MIXED_PLAINTEXT_WIRE_FORMAT_POLICY)` on
   the `MlsGroupJoinConfig::builder()` so freshly-joined groups
   start with the right policy out of the gate.
2. `add_members_commit` calls a private
   `ensure_plaintext_handshake` helper that loads the group, and
   if its persisted `wire_format_policy` is anything other than
   MIXED_PLAINTEXT, rebuilds the join config with MIXED_PLAINTEXT
   and persists via `MlsGroup::set_configuration`. Idempotent:
   no-op for already-MIXED groups. This back-fills any group
   joined under a previous Kupferdraht build.
3. `add_members_commit` also calls
   `MlsGroup::clear_pending_proposals` before staging the Add.
   Sync-fetch can deposit peer-originated proposals (notably
   stale Removes against prior device leaves of the same user)
   into the local proposal store; without clearing, openmls
   bundles them into our Add and trips
   `CreateCommitError::CannotRemoveSelf`. Kupferdraht's commit
   semantics are intentionally atomic-per-call ("an invite adds
   only the invitee"), so dropping queued peer-proposals matches
   the contract. The trade-off is documented inline: a future
   peer Commit that references a cleared proposal will not
   apply locally — Kupferdraht does not yet round-trip pending peer
   proposals, which is acceptable for Step 6's scope.
   *(Follow-up 2026-08-16, Step 71 / gap-B.20: the blanket clear
   also discarded queued backend external Removes — a Step-28
   commit obligation. The clear is now selective: Removes
   targeting peer leaves stay queued and ride the user's commit;
   only own-leaf Removes — openmls checks `self_removed` by leaf
   index, verified in `commit_builder.rs` — and non-Remove
   classes are dropped. openmls dedups per-leaf Removes, so a
   kick of an already-externally-removed leaf builds fine.)*
4. The earlier-mistaken epoch-based dispatch in
   `add_members_commit` is removed. Every Add commit now uses
   `add_members_without_update` regardless of epoch.

The matching update in `docs/wire/api-notes.md` removes the
"empty path on a commit Wire requires to have one" reading of
"Missing commit" — that was a misinterpretation of the symptom.

Live-validated 2026-04-29 against Wire production: room
`Kupferdraht 6c smoke 134340` advanced epoch 0 → 1 on the second account's
invite (epoch-0 path), then epoch 1 → 2 on Markus's invite
(epoch-1 path, the actual Step 6c case); `room-members` showed
3 members; `kupferdraht-cli send` accepted application messages.

---

## 31) Wire MLS self-leave is server-driven via REST DELETE, not a client-built self-Remove commit (2026-04-30)

Discovered while implementing Step 6e-1 (`Room::leave`). Initial
approach was to mirror Step 6d's kick path: build a Remove-commit
targeting our own `LeafNodeIndex` and upload to
`POST /v14/mls/commit-bundles`. openmls refused with
`CreateCommitError::CannotRemoveSelf`.

Verified against Kalium's
`ConversationGroupRepository::deleteMemberFromMlsGroup` + 
`MLSConversationRepository::leaveGroup`: when the target user is
`selfUserId`, Kalium calls

* `DELETE /v{N}/conversations/{cnv_d}/{cnv}/members/{usr_d}/{usr}`
  (`conversationApi.removeMember`), and then
* `mlsContext.wipeConversation(group_id)` — local-only cleanup,
  no MLS commit.

Wire's external-sender then produces the MLS Remove on the
leaver's behalf and the remaining members receive that commit
via their `/v14/notifications` stream. The leaving client never
sees it because they've already discarded the local group.

This is the correct pattern: openmls's `CannotRemoveSelf` is a
deliberate guard rail — RFC 9420 §12.1.2 only allows Remove
proposals for self-leave (someone else commits them), and Wire
puts the "someone else" responsibility on the server.

**Kupferdraht adopts the same flow:**

1. `HttpClient::delete_conversation_member(room_id, target_user_id)`
   — generic DELETE wrapper (works for self-leave with
   `target = self_user_id`; future kick-via-REST paths can reuse it
   if Wire ever requires it for proteus rooms).
2. `Client::leave_room` — calls the DELETE, then on success calls
   `MlsClient::discard_group` for local cleanup, then runs
   `sync_metadata` so the rooms cache forgets the now-left room.

No `MlsClient::leave_group_commit` exists — there is no client-side
MLS work to do for self-leave.

Implication for Step 6e-2 (`Client::logout`): same pattern likely
applies — `POST /v14/access/logout` revokes the access token,
optional `DELETE /v14/clients/{id}` revokes the device. No MLS
work; Wire handles the cascade server-side.

---

## 33) Wire's Quote.quoted_message_sha256 is `optional` on the wire but quasi-required for rendering (2026-04-30, fixed in 6-II-b)

Live-validated during Step 6-II-b's smoke. Kupferdraht emitted a
valid `Text.quote { quoted_message_id }` and Wire's transport
accepted it; Wire web rendered it as a plain message anyway.
Inbound replies from Wire web *did* reach Kupferdraht with the
`quoted_message_id` populated, so the receive direction was
correct; only the outbound rendering was broken on the
counterpart.

**Root cause:** Wire's protobuf marks
`Quote.quoted_message_sha256` as `optional`, but Wire's clients
(web, iOS, android — Kalium's
`MessageSendingInterceptor::prepareMessage` in
`logic/src/commonMain/kotlin/com/wire/kalium/logic/feature/message/MessageSendingInterceptor.kt`)
always populate it, and the renderer treats absence as "this is
not a real reply, render flat". Kupferdraht's earlier omission left
the field default-empty and Wire web silently downgraded the
quote to a plain message.

**Format** (Wire wire-format, observed from inbound quotes that
Wire web *does* render — see
[ADR-0008](docs/adrs/0008-wire-api-v14.md) on externally
constrained shapes):

```
sha256( 0xFE 0xFF || body_utf16_be || timestamp_seconds_be )
```

Where:
* `0xFE 0xFF` is a literal two-byte UTF-16 BOM.
* `body_utf16_be` is the parent message's plaintext encoded as
  UTF-16 big-endian (no separate BOM beyond the prefix above).
* `timestamp_seconds_be` is the parent's send-time floored to
  seconds, encoded as a big-endian i64 (`(sent_at_ms / 1000)
  .to_be_bytes()` in Rust).

**Fix:** `wire/proto.rs::compute_quote_sha256` does the SHA-256;
`Client::send_text` looks up the parent's `body` + `sent_at_ms`
in `timeline_events` and threads them via the new
`QuoteParent` struct into the protobuf builder. The digest
travels in the existing `Quote.quoted_message_sha256` field
(now declared in `proto/generic_message.proto`).

**Pre-condition for replies:** the parent must already exist
in `timeline_events` at send time. Today we look it up from
the local store (`Client::lookup_timeline_event`); if the
parent isn't persisted (e.g. the user is replying to a
message that arrived *before* the V9 CASCADE fix) we fall
back to emitting the Quote without a digest, and Wire renders
flat — that's a graceful degradation, not a crash. A future
"server-side history backfill" path (Step 8 territory) would
remove this caveat.

**Implication for Step 6-II-c (Edit) and 6-II-d (Delete):**
those use `MessageEdit.replacing_message_id` and
`MessageDelete.message_id` references that **do not** require
the same digest scheme — Wire's Edit / Delete flows look up
the parent by id alone. Quote is the odd one out here.

---

## 34) Wire's MessageEdit chains via the previous edit-event-id, not the original message_id (2026-04-30, fixed in 6-II-c)

Live-validated during 6-II-c smoke. Kupferdraht's first inbound edit
landed correctly (the row's body was replaced); subsequent
edits on the same message in Wire-Web showed up at Kupferdraht with
"MessageEdit had no effect" warnings. The transcript:

```
MessageEdit received  replacing="63ff26f0-…"  → had no effect
MessageEdit received  replacing="a663692c-…"  → had no effect
```

Neither id matched any row in `timeline_events`. The user only
edited one local message (`2491965d-…`) but Wire-Web's
`replacing_message_id` pointed at unknown ids.

**Root cause:** observed in Kalium's edit handler
([`MessageTextEditHandler.kt:84`](https://github.com/wireapp/kalium/blob/9ccecabb04742ec0afa30babe9925bea7313a17c/logic/src/commonMain/kotlin/com/wire/kalium/logic/sync/receiver/handler/MessageTextEditHandler.kt#L84)):
applying an edit stores the edit event's id as the row's new
message id.

Kalium **rewrites** the persisted row's id on every edit — the
chain is `original.id → edit1.id → edit2.id → …`. Each
successive `MessageEdit.replacing_message_id` references the
*previous* edit-event's id, not the original message's id.
Kupferdraht's Step 6-II-c v1 only matched against the original
`message_id` in SQL; chained edits silently fell off.

**Fix (V12 + chain pointer):** Kupferdraht keeps the row's
`message_id` PK stable (so consumers can track items across
edits and `reply_to_id` references stay valid) and tracks the
chain in a new `current_edit_event_id` column. After every
`apply_message_edit`:

```sql
UPDATE timeline_events
   SET body_ct               = ?new_body,
       edited_at_ms          = ?ts,
       current_edit_event_id = ?edit_event_id
 WHERE (message_id            = ?replacing
        OR current_edit_event_id = ?replacing)
   AND room_id               = ?room
   AND (edited_at_ms IS NULL OR edited_at_ms < ?ts);
```

The OR clause covers both the first edit (matches original
message_id) and every subsequent chain link (matches the
previously-stored event id). The strict `< ?ts` keeps the
idempotence guard against redelivery.

The signature gained `edit_event_id: &str`. Both the outbound
path (`Client::edit_message`, has the id from
`build_message_edit_generic_message`) and the inbound path
(`generic.message_id`) feed it through.

**Why not Kalium's approach (rewrite the PK)?** Two reasons:

1. `Room::messages` is a public API; consumers tracking items
   by `item_id` would see the id mutate every edit. matrix-rust-sdk
   keeps event ids stable too — Kalium's approach is a Wire-
   specific implementation detail, not a general pattern.
2. `reply_to_id` references the parent's `message_id`. If the
   parent's id mutates on every edit, every reply pointing at
   it also breaks. The chain pointer in a separate column
   sidesteps this.

**Caveat (data migration):** rows that received edits *before*
V12 have `current_edit_event_id = NULL` permanently — the
previous edit-event-id was never stored. Their next chain edit
cannot be matched. Going forward (after V12), every fresh edit
chain works end-to-end. Old chains effectively "stop here"
until a new message is created.

The 6-II-c smoke transcript that revealed this is the test that
validates the fix going forward — the user's `2491965d-…` row
was already past V11 (one edit applied, no chain pointer),
hence the loss of subsequent edits before V12.

---

## 36) `POST /v14/conversations/{cnv}/code` returns Event-on-201 / `ConversationCodeInfo`-on-200 (2026-05-04)

Live-smoke against Wire production for Step 8's `Room::create_join_link`
surfaced the contract: the swagger documents both responses, but
inconsistently with the surface of `GET /code`.

* `200 OK` — code already existed for this conversation; body is the
  full `ConversationCodeInfo` envelope (`{key, code, uri, has_password}`).
* `201 Created` — code was newly issued; body is **not**
  `ConversationCodeInfo`. Wire returns an `Event` (a
  `conversation.code-update` member-update notification with no link
  material). To retrieve the actual `key`/`code`/`uri`, the client must
  immediately follow up with `GET /v14/conversations/{cnv}/code`.

The first Kupferdraht implementation parsed every 2xx as
`ConversationCodeInfo` and surfaced `Encoding(conversations/code body:
error decoding response body)` on the newly-created path while the
already-exists path worked. Took two smoke iterations to nail down (200
worked because the room already had a code from an earlier accidental
test; revoke + retry tripped the 201 path).

**Fix:** `HttpClient::create_conversation_code` branches on status —
on 201, it drains and discards the Event body, then calls
`get_conversation_code` immediately for the canonical envelope. On
200, it parses the body as `ConversationCodeInfo` directly. Either way
the caller sees the same `JoinLink` projection. Locked in by
`room_create_join_link_201_follows_up_with_get_to_fetch_envelope` in
`tests/step_8_join_links.rs`. Commit `d97ae35` / merge `b973c07`.

**Lesson:** Wire's swagger lists multiple 2xx return shapes per
endpoint and they really differ — don't assume the swagger's "primary"
schema covers every status. Branch on status code at the HTTP layer.

---

## 37) Kupferdraht-created conversations default to `access=["invite"]` — `POST /code` rejects with `403 access-denied` until `PUT /access` opens them (2026-05-04)

`Room::create_join_link` against an existing Kupferdraht-created group
conversation surfaced `403 access-denied` — Wire's "you're allowed to
do *this* but the conversation isn't configured to permit it" answer.
Wire's group-conversation defaults set `access=["invite"]`,
`access_role=["team_member","non_team_member","service"]`. The `code`
and `link` access modes are opt-in per-conversation: until
`access` includes `"code"` (and `access_role` typically gains
`"guest"`), `POST /code` returns 403 regardless of the caller's role.

The Wire web client gates the equivalent "Allow guest links" toggle in
the conversation settings, which under the hood emits a
`PUT /v14/conversations/{cnv_domain}/{cnv}/access` with
`access=["invite","code"]`,
`access_role=["team_member","non_team_member","guest"]` before issuing
the code.

**Fix (Step 8b, branches `step/8b-room-access` + `adf974b`):**

* HTTP layer: `HttpClient::set_conversation_access(api, token, room_id,
  access, access_role)` PUT'ing to the access endpoint.
* Public API: `Room::access_modes()` / `Room::access_roles()` read-only
  accessors (sourced from `OwnConversation`'s `access` /
  `access_role` arrays which Kupferdraht hadn't deserialized previously);
  `Room::set_access(modes, roles)` and `Room::open_to_guests()`
  (canonical `["invite","code"]` /
  `["team_member","non_team_member","guest"]` configuration);
  `Client::set_room_access` / `Client::fetch_room_access` (single-id
  `POST /conversations/list` re-fetch for the read path).
* CLI: `room-access-show`, `room-access-set --modes A,B --roles X,Y`,
  `room-access-allow-guests`. Consumer flow:

  ```
  room-access-allow-guests --id <room>
  join-link-generate --id <room>
  ```

`Room::access_modes` / `access_roles` aren't persisted to the SQLite
store; they live in the `OwnConversation` snapshot and refresh on
every `sync_metadata`. `room-access-show` therefore goes through
`Client::fetch_room_access` (live re-fetch) so the CLI output reflects
Wire's current state regardless of cache freshness.

**Lesson:** Wire's per-conversation access mode is independent of the
caller's role. Steps that touch any guest-link / public-link surface
need to either expose the access-mode controls explicitly or
auto-enable the right defaults — without this the surface is
unreachable for any conversation Kupferdraht itself created.

---

## 38) Wire's `POST /code` returns `409 create-conv-code-conflict` if a code already exists with a different password setting (2026-05-04)

Smoke followed up on §36 by attempting `join-link-generate -p` (issue
a password-protected link) on a conversation that already had an
open guest link from the prior test. Wire responded with `409 Conflict
{"label":"create-conv-code-conflict","message":"Conversation code
already exists with a different password setting than the requested
one."}`. This is by design — there's only one code per conversation,
and switching between open ↔ password-protected requires explicitly
revoking first.

The error surfaces with the informative `label` intact through
Kupferdraht's `BackendRejected` mapping, so callers see the actionable
message without Kupferdraht needing a special case. Documented as expected
behaviour; the Owner-side smoke flow is therefore:

```
join-link-revoke --id <room>
join-link-generate --id <room> [-p]
```

No code change. Recorded so the next Step-8 reviewer doesn't chase it
as a bug.

---

## 59) Wire self-leave delivers an external-sender Remove *proposal*, not a ready-made commit — remaining member must commit it (2026-06-15)

Live-confirmed during the Step 28 smoke against Wire production. Setup:
accounts A and B, both members of a shared group. B
self-leaves (`DELETE /v14/conversations/{d}/{cnv}/members/{d}/{B}` via
`room-leave`). A then `sync`s.

A's inbound `conversation.mls-message-add` for that group decodes to an
openmls `ProcessedMessageContent::ProposalMessage` (a Remove proposal
signed by the backend external sender) — **not** a `StagedCommitMessage`.
This is exactly the path Step 28 added: `process_inbound_message`
returns `DecodedMessage::ProposalQueued`, and
`Client::commit_pending_external_removal` commits + uploads the bundle.

Observed marker on A's sync (RUST_LOG=kupferdraht=info):

```
Step 28: committing external-sender Remove proposal (peer removed)
```

Outcome: B dropped from A's roster and A stayed sendable (epoch advanced
cleanly). Confirms `docs/backend-spec.md` §10.5 path-2 (the backend
issues a Remove *proposal*; a member commits it) and validates the
ADR-0004 auto-commit obligation. Before Step 28 the proposal was stored
then discarded, leaving B as a ghost leaf.

Branch `step/28-external-proposal-autocommit`.

---

## 61) Self-update establishing commit advances a fresh group epoch 0 → 1 — `create_room` is sendable immediately (supersedes §25, 2026-06-16)

§25 concluded a freshly-created zero-invitee room is unavoidably inert
at epoch 0 until the first `Room::invite`. That is wrong. Wire accepts a
**self-update** ("update keying material") commit as the
group-establishing commit — the same way Wire's own client brings a solo
conversation online. Wire-API/client protocol observation confirmed the
pattern: on create, claim the creator's own remaining clients'
KeyPackages (`skip_own` the current device); if any exist → an
Add-commit; if none (solo / single device) → a self-update commit.
Either is uploaded to `POST /v14/mls/commit-bundles` and advances epoch
0 → 1.

Why §25's three attempts failed (re-diagnosed):

- *"empty `LeafNodeParameters` → Missing commit"*: the commit was
  `PrivateMessage`-framed; Wire's bundle parser only reads
  `PublicMessage` commits and folds the rest into "Missing commit".
  `MIXED_PLAINTEXT_WIRE_FORMAT_POLICY` (already on the create config, and
  what the Add path relies on) fixes this — the self-update path was
  abandoned before it benefited.
- *"explicit credential params → Client identity does not match"*:
  passing changed credential params re-signed the leaf. The correct call
  is a plain `self_update` with **default** `LeafNodeParameters` (no
  credential change).
- *"`force_self_update(true)` → mls-client-mismatch"*: not needed; a
  plain `self_update` keeping the same credential is what Wire expects.

Resolution: `create_room` now runs `Client::establish_group` right after
the local `create_group` — claim-`skip_own` → Add (own other devices) or
`MlsClient::self_update_commit` (solo) → `upload_and_merge_commit_bundle`
— so the room is at epoch 1 and immediately sendable. Live-validated
against Wire production (2026-06-16): `room-create` reports `epoch: 1`
and a subsequent `send` is accepted (no `mls-protocol-error`). Unit test
`self_update_commit_produces_a_mergeable_public_message_commit` pins the
`PublicMessage` framing + mergeability.

Branch `step/31-self-establishing-group`.

## 62) Wire v14 exposes no key-backup / recovery endpoint — mnemonic backup (Step 12) is blocked on the backend (2026-06-30)

The v14 Swagger carries no key-backup, recovery-key, or mnemonic-restore
surface, and none was observed live. A backup / restore feature (Step 12)
therefore has no endpoint to target — it is blocked on Wire adding the
surface, not a Kupferdraht-side omission. Tracked as a blocked backlog item in
`open.md` (Backlog → Blocked on Wire).

## ~~63) Wire v14 exposes no dehydrated-device endpoint — Dehydrated Device (Step 13) is blocked on the backend (2026-06-30)~~

*Closed 2026-09-25 — Step 13 dropped from the plan (no longer needed); the finding stays true. Text kept unchanged as the findings log.*

No dehydrated-device (DD) endpoints appear in the v14 Swagger or were
observed live. Step 13 (Dehydrated Device — an offline-provisioned spare
device) is therefore blocked on Wire, not a Kupferdraht gap. Tracked in `open.md`
(Backlog → Blocked on Wire).

## 64) Self-conversation has no local MLS group — sends fail with MlsGroupMissing (2026-07-17)

Verified in the 60c live smoke: the self conversation (Wire
`room_type: self_room`, server-side epoch 40) exists in the rooms
snapshot but has no local MLS group state — `task add` (any send)
fails with `MlsGroupMissing`. The failed send still persists its
local-echo row (`delivery_status` failed), so the task shows in
`task list` without ever leaving the device — correct local-echo
behavior, but easy to misread as a successful send. Joining the
self conversation via external commit was not attempted; whether
Wire accepts an external commit into a self room is unknown.
Establishing the self conversation (the personal task list's home,
ADR-0014) is its own step — tracked in `docs/plan/archive/60-tasks.md`
follow-ups and `open.md`.

## 65) Tasks-as-runtime-extension considered and rejected; content extensions are compile-time vertical slices (2026-07-18)

Post-merge discussion of Step 60: could tasks live outside
`kupferdraht-core` as a loadable extension, motivated by Kalium-copyleft
caution? Rejected on three grounds. (1) The licensing motivation
inverts: copyleft risk follows code *provenance*, and tasks have no
Kalium/Wire counterpart to copy — they are the cleanest-provenance
code in the repo; the GPL-sensitive surface is the Wire-protocol
layer, which cannot be an extension. (2) Runtime plugins in an E2EE
client mean third-party code over decrypted content — a threat-model
expansion Signal/Element/Wire all deliberately avoid — plus
`dlopen`/unsafe (forbidden), no wasm story, and UniFFI cannot export
dynamically registered types. (3) The single-crate/no-internal-FFI
decision (ADR-0009) stands. What the idea *is* worth: tasks already
form a compile-time **vertical slice** (non-oneof proto field, fold
module, store methods with no-op defaults, own migration,
`timeline/task/` folder) — that pattern, not a plugin framework, is
how future content types (polls, whiteboards) should land. An
optional `tasks` cargo feature for opt-out builds remains possible if
a consumer ever needs it.

## ~~66) `/v14/events` live findings: binary frames, acked markers — and gundeck publishes nothing (2026-08-12)~~

*Closed 2026-09-24 — root cause found in §68 (hard-disabled upstream since wire-server 5.30); ADR-0019 auto-negotiates the transport. Text kept unchanged as the findings log.*

Step 62's smoke against the self-hosted v14 instance (current
wire-server) surfaced three deviations from the frozen backend-spec
§11.2/§11.3 contract (which was live-derived from Wire production in
2026-04):

1. **Frames arrive as binary WebSocket frames**, not text — the JSON
   payload is unchanged. Kupferdraht now accepts both (`wire/websocket.rs`).
2. **`synchronization` frames carry a `delivery_tag` and ride the
   durable queue**: they must be acked like events, or the broker's
   prefetch window stalls the stream after a handful of unacked
   deliveries (observed live: two unacked markers froze all further
   delivery). Stale markers from earlier connections replay on
   reconnect — ack them, but only the own echo is the "caught up"
   signal worth persisting.
3. **No event of any class is ever published to the consumable
   queue** — neither durable (`conversation.mls-message-add`) nor
   transient (`conversation.typing`) — while the same events reach the
   REST `/notifications` store and drain fine. The socket side is
   healthy end-to-end (101, marker enqueue/echo, acks accepted,
   redelivery semantics correct), which isolates the gap to the
   **server's event fan-out into RabbitMQ** (gundeck-side; cannon-side
   works). Same symptom as Wire production in CHANGELOG §30 — so §30's
   silence was likely this server-side switch too, not account
   configuration. Root cause found in wire-server: the root-level Helm
   flag **`consumableNotifications`** (rendered into brig
   `optSettings.setConsumableNotifications` and gundeck
   `settings.consumableNotifications`) **defaults to `false`** — "when
   disabled, the legacy notification flow remains active"
   (wire-server `docs/.../config-options.md`), i.e. cannon serves the
   socket and injects markers but gundeck never publishes events into
   the queue. Ruled out client-side: the v14 swagger and Kalium's
   `NotificationApiV9` both send only `client` + `sync_marker` on the
   upgrade — no extra parameter exists (Kalium's `app` parameter
   belongs to `POST /push/tokens`, mobile push only).
   **Client-side Kupferdraht is protocol-complete; the smoke is blocked on
   backend ops** (set `consumableNotifications: true`, then re-run
   `scripts/smoke-step62.sh`; a team feature
   `ConsumableNotificationsConfig` exists as a secondary gate).
   *(Resolved 2026-08-18: not an ops knob — wire-server hard-codes
   the flag to `false` in its chart templates since 5.30, with no
   public timeline for re-enabling it; the legacy `/await` socket carries live push instead. §68.)*

The client's registered `consumable-notifications` capability was
verified server-side (echoed by `GET /v14/clients`). Also observed:
`create_dm`'s MLS 1:1 conversation (deterministic v5 id) does not
appear in `/conversations/list-ids` on this instance after the
one2one fetch — untracked so far, needs its own investigation (the
Step-62 smoke switched to an existing shared group room).
*(Follow-up 2026-08-14: root-caused + fixed by Step 68 — the fetch
only reserves the 1:1; it stays hidden until the establishing
commit bundle is uploaded. `create_dm` now establishes on epoch 0
and external-joins on epoch > 0; contract recorded in
`docs/wire/api-notes.md` "Step 68 discovery".)*

## 67) Step-68 live smoke: establishment works — and exposes the Welcome gap's real cost (2026-08-14)

Three `smoke-step68.sh` runs against the own instance
(schwarz-dev-wire), fresh devices per run:

1. **Epoch-0 establishment live-verified.** `dm-create` claimed the
   peer's key packages, uploaded the Add-commit, epoch 0 → 1, and the
   1:1 **appeared in `list-ids` immediately** — the §66 loose end is
   closed. Re-runs against the now-established conversation took the
   `epoch > 0` external-join branch (epoch 3, then 5) and the
   local-group resume branch (idempotent second call, no re-claim,
   no re-upload); all three `create_dm` branches proven live.
2. **The unabsorbed Welcome now has a visible cost.** The claimed
   peer device received `conversation.mls-welcome`, logged the Step-6
   "can't yet absorb peer-initiated invites" warning, skipped it, and
   only became a member when the out-of-order heal (or
   `rooms-join-all`) external-joined it — *after* the initiator's
   first message. By MLS forward secrecy that first message is
   unreadable for the peer forever. Real Wire clients absorb the
   Welcome and read it. Tracked as `open.md` gap-B.24.
   *(Follow-up, same day: closed by Step 69 — the drain absorbs the
   Welcome via `StagedWelcome` and fires `on_room_invited`;
   live-verified with the pre-sync first message read.)*
3. **A non-member device is silent, not broken.** A fresh device of
   the peer (post-establishment run) sees the conversation metadata
   via `list-ids` but receives **no** message events — Wire fans MLS
   messages out per *group leaf*, not per conversation member. No
   undecryptable traffic arrives, so the out-of-order heal never
   fires; membership comes only from the explicit `rooms-join-all`
   bootstrap. A consumer that skips the bootstrap sees a permanently
   quiet room with plausible metadata.

## 68) ConsumableNotifications is hard-disabled upstream since wire-server 5.30 — the legacy `/await` socket is the live push channel (2026-08-18)

**Upstream fact.** wire-server PR #5116 (WPB-23942 *"prevent rabbit mq
queues creation for client notifications"*, merged 2026-03-23, chart
5.29 — itself broken — effective 5.30, 2026-04-20) added a
`consumableNotifications` flag to brig (`setConsumableNotifications`)
and gundeck and **hard-codes it to `false` in the chart templates**
(`charts/wire-server/templates/{brig,gundeck}/configmap.yaml`) — not a
value an operator can flip; PR #5116 hard-codes it, with no public
timeline for re-enabling it. Consequence in gundeck's
`Push.hs`: with the flag off, `splitPushes` is bypassed and **every**
push goes the legacy route (cannon `/await` WebSocket + Cassandra
`/notifications`), regardless of the client's
`consumable-notifications` capability. That is why the Step-62 socket
connected, marked and acked perfectly but never received an event
(§66) — and why the "ops flag" theory was only half right: the flag
exists, but only a chart patch could turn it on.

**Spike against the own instance (wire-server ≥ 5.30), 2026-08-18,
Python `websockets`, three connects:**

1. `wss://nginz-ssl.<domain>/await?client=<device>&access_token=<t>`
   → `101` in 0.5 s. User 3 sends into the shared DM room → **0.8 s
   later one binary frame** on user 1's socket. Content is JSON —
   the notification envelope **identical to a `/v14/notifications`
   item**: `{"id": "<time-uuid>", "payload": [<event>], "transient":
   false}` with the full `conversation.mls-message-add` event
   (`qualified_conversation`, `qualified_from`, `data`, `time`,
   `via`). So §15's April diagnosis ("binary frames, JSON parse
   fails") was a client-side struct mismatch: cannon frames JSON in
   **binary** WebSocket frames, same as `/v14/events` (§66).
2. `Authorization: Bearer <t>` header instead of the query token →
   `101` in 0.1 s, same delivery. Transient typing (REST `POST
   …/typing` as user 3) arrives live as `"transient": true` with the
   `conversation.typing` payload — the transient class polling can
   never see.
3. **Without `client=`** the socket still opens and receives the
   user's events (user-scoped delivery).

**Design consequences (recorded for Step 73):** the socket is
*passive* — no ack, no sync marker; the frame's `id` is the same
notification id the REST backfill pages by, so a socket-delivered
notification advances the persisted cursor and a reconnect gap is
closed by the existing `/v14/notifications?since=<cursor>` drain
(Step-64 dedup + 404 recovery already handle the edge cases). Not
probed: server ping/idle-timeout behaviour on the legacy route
(cannon `wsOpts` apply — keepalive as in Step 62), and the exact
`client=`-less semantics for multi-device users; both belong to the
step's smoke.

*Corrected 2026-09-28 (Step 97b, ADR-0030):* the drain before the
connect closes only the gap before the drain, not the window between
the drain and the socket registration — and a frame that advances
the cursor makes any push the socket misses permanent (`since` is
exclusive). The frame is now only a doorbell: connect first, then
drain; each durable frame triggers a drain; only the drain advances
the cursor. The shared id space above still holds.

*2026-10-08: the wording of §68 follows the public record — PR #5116
hard-codes the flag to `false`, so the legacy `/await` socket carries
live push.*

## ~~69) Device deletion seeds Remove proposals that can wedge old groups (2026-08-21)~~

*Closed 2026-09-24 — superseded by §70 (groups heal; the defect was the commit storm), fixed by Step 74. Text kept unchanged as the findings log.*

The Step 73 live smoke first ran against the old shared smoke rooms
and hit a hard wall in stage 2: after the 2026-08-21 cleanup of 14
stale devices (Wire's ~8-devices-per-account cap), every old group on
the instance carries a backlog of backend-queued external Remove
proposals — one per deleted device. Joining such a room from a fresh
home now fails permanently: the drain loops on `upload_commit_bundle`
400 `mls-commit-missing-references` ("The commit is not referencing
all pending proposals"), the group never leaves epoch 0 for the new
client, and sends die with `mls-protocol-error` ("Application
messages at epoch 0 are not supported").

Mechanism (hypothesis, not yet verified in code): the Step-28
auto-commit fires per *received* Remove proposal, so each commit is
uploaded before later queued proposals have arrived and Wire rejects
it for not referencing them; an external join cannot reference queued
proposals at all (RFC 9420 external commits carry no by-reference
proposals), so a backlogged group may be unjoinable until an existing
member with an established group commits the whole backlog. Tracked
as `open.md` gap-B.25 (batch-drain before auto-commit + a join story
for backlogged groups). The Step 73 smoke sidesteps it with a fresh
room; the old smoke rooms on the instance stay wedged until a member
commits their backlog.

Operational note: deleting devices is not free — every deletion
touches every group the device was in. Clean devices *before*
creating long-lived smoke fixtures, not after.

## ~~70) gap-B.25 re-probed: the backlogged groups healed themselves — the defect is the storm, not a wedge (2026-08-24)~~

*Closed 2026-09-24 — resolved by Step 74 (batched removal commits, signal-then-flush); residual gap-B.25 (restart loses the in-memory pending set). Text kept unchanged as the findings log.*

Follow-up to §69 before starting Step 74. Code verification first:
the Step-28 auto-commit path (`sync/service.rs::decrypt_to_application`
→ `Client::commit_pending_external_removal`) commits per received
Remove proposal, and openmls 0.8.1's `clear_pending_commit` (read in
the registry source) resets only the group state — **the proposal
store survives a failed upload**. So during an N-proposal backlog
drain, commits for 1..k are rejected (`mls-commit-missing-references`)
but proposals keep accumulating, and the commit built after proposal
N references all of them and is accepted: the 2026-08-21 warn-loop
was the storm *converging*, not a permanent failure.

Live re-probe with the surviving `smoke73` home (fresh device,
2026-08-24): `sync` drained 2 leftover notifications,
`rooms-join-all` entered **7/9 old rooms via external commit** (1
already joined; 1 failed with 404 `mls-missing-group-info` — a
conversation that was never MLS-established, which also explains the
first smoke run's "Application messages at epoch 0" send failure:
an unlucky `head -1` room pick, not proposal damage), and a probe
send into a re-joined old room succeeded. Whether Wire would accept
an external join *while* a backlog is pending remains unproven —
in practice established members converge the backlog first.

What remains of gap-B.25 (Step 74 scope): the N−1 wasted uploads +
consumer-visible `on_sync_error` spam per backlog, and the
no-re-trigger window when the final commit fails transiently
(cursor already advanced). Fix: signal-then-flush per dispatch
cycle, rooms stay pending until a commit succeeds,
`mls-commit-missing-references` downgraded to debug on the push
path where mid-backlog rejections are expected.

## ~~71) Task-cache backfill trigger sits on a path sync-metadata consumers never take (2026-08-24)~~

*Closed 2026-09-24 — resolved — `backfill_task_cache` now runs on the sync path (`client/sync.rs`), not only on `load_rooms_from_store`. Text kept unchanged as the findings log.*

`backfill_task_cache` (V23 one-time backfill: rebuild the
materialized `tasks` table from `timeline_events` when the table is
empty) is called ONLY from `Client::load_rooms_from_store`. A
consumer that hydrates via `sync_metadata` (kupferdraht-tui does — the
zero-arg live path) never triggers it: a store whose `tasks` table
is empty or gappy while task rows exist in `timeline_events` stays
that way. Healing through new traffic is narrower than it looks:
the dispatch path upserts only the folded record of the task the
event belongs to (`sync/dispatch/task.rs`), and only a task
*redaction* rebuilds a whole room's cache
(`rebuild_tasks_if_redacted_task`) — other missing rows stay
missing. Surfaced while chasing a kupferdraht-tui report ("views empty
after restart" — not yet reproduced; the send/dispatch upserts and
restart persistence verified fine in three live runs on
test1/test2). Candidate fix if it bites: run the backfill from
`ClientBuilder`/store-open or from `sync_metadata`'s first pass,
not from the optional hydrate helper.

## 72) ADR-0018's invite re-broadcast silently fails for non-admin inviters (2026-08-24)

Found while designing Step 75's late-join story against the ADR-0018
precedent. `Client::invite_members` re-broadcasts the case snapshot
unconditionally (`membership.rs`, the 72c MUST) — but on the late
joiner's side the fold (`case/fold.rs::apply_case_event`) discards a
non-admin sender's snapshot entirely when no folded case exists yet:
`sender_was_admin == false` + `current == None` contributes nothing,
by design (a first-snapshot exception would let any member forge
case state for exactly the people who cannot check it). Wire lets
ANY member invite, so a non-admin's invite fulfils the MUST
mechanically while the new member still ends up without case state.
Neither ADR-0018 nor `rules/0018` named this trade-off at
acceptance; both now do (0018 decision 3 + Consequences, amended
2026-08-25). Mitigations short of weakening the gate: none
client-side (the gate is the point); the state heals on the next
admin core update. Step 75's marker avoids the class entirely
because its authorization is any-member.

## 73) openmls 0.9's storage-format break does not bite JSON codecs — and the 0.8.1 compat behaviors, source-verified (2026-09-09)

Step 76 (openmls 0.8.1 → 0.9.0) went in expecting the changelog's
storage-format trap: "storage format compatibility with v0.7.1
restored by default; the `0-8-1-storage-format` feature maintains
v0.8.1 compatibility" — read as "without the feature, 0.8.1-written
state is unreadable". A negative probe said otherwise: Kupferdraht's
committed 0.8.1 fixture mls.db (tests/fixtures/mls-081, plain and
SQLCipher) opened fine under 0.9 WITHOUT the feature. Source
verification explained it:

- The feature toggles the **serde representation of specific enums**
  (`CredentialType`, proposal/extension types): with the feature,
  plain serde derives (the 0.8.1 form); without, the new
  `openmls_serialization_helpers` derives with explicit
  `storage_tag` numbers (the 0.7.1-and-forward form).
- The helper macros route through serde's standard variant API
  (`serialize_unit_variant(name, tag, variant_name)`). **Self-
  describing codecs (JSON) serialize the variant NAME either way**;
  the `storage_tag` only manifests in non-self-describing codecs
  (bincode and friends). The helper deserializer additionally
  accepts BOTH numeric tags and names.
- Kupferdraht's `SqliteStorageProvider` codec is JSON (`JsonCodec`,
  provider.rs — chosen in Step 1c.5 as "a safe default"). For our
  databases both feature states therefore read and write identical
  bytes: the entire 0.7.1/0.8.1 format break is a bincode-class
  problem that never applied to us.

The feature stays enabled anyway (declares the 0.8.1 history;
future 0.9.x patches may add feature-gated compat points), but the
real contract is the fixture roundtrip pins. Two more 0.9 findings
from the same pass:

- `clear_pending_commit` (0.9 source, mls_group/mod.rs) still
  touches ONLY `group_state` (PendingCommit → Operational) — the
  proposal store survives, so Step 74's signal-then-flush batching
  assumption holds unchanged (re-verification of the 0.8.1 reading
  in §~1170 above).
- `ProcessedMessageContent` gained `OwnPendingCommit` (own
  public-framed commit fanned back, matched by confirmation tag —
  correct action: merge the pending commit) and `OwnPrivateMessage`
  (own private message fanned back, undecryptable by design —
  documented skip hint). Wire's per-client notification streams
  normally exclude the origin client, so both are defensive arms in
  Kupferdraht (`encryption/messages.rs`).

*2026-10-08:* the fixtures and the `0-8-1-storage-format` feature were
retired (CHANGELOG, Removed): ADR-0041 left no 0.8.1-format database
behind, and the fixtures bound a real device identity. The finding
above stands on its own.

## 74) Ephemeral messages: proto shape, where the timer starts, and who enforces expiry — verified against Wire's public proto + wire-webapp (2026-09-15)

Step 89 orientation, protocol observation only (nothing copied).
Sources: Wire's public `generic-message-proto` schema and the
wire-webapp repository (GPL-3.0), both read on 2026-09-15; the
wire-webapp files consulted were not recorded:

- **Proto shape confirmed.** `GenericMessage.content` field **18** is
  `Ephemeral { required int64 expire_after_millis = 1; oneof content
  { Text text = 2; ImageAsset image = 3 (deprecated); Knock knock =
  4; Asset asset = 5; Location location = 6; } }` — proto2, matching
  our local `syntax`. Kupferdraht declares only `Text` (V1); the other
  arms decode to `content = None` and take the tolerated-unknown
  path (pinned in `wire/proto.rs`).
- **The timer is purely client-side.** The server never sees or
  enforces the TTL — every client counts down and deletes its own
  copy. Consequence: nothing stops a non-conforming client from
  keeping the plaintext; the feature is a UX convention, not a
  confidentiality guarantee (relevant to one-time-code delivery: the code's
  security comes from its single-use + server-side expiry, not from
  the message vanishing).
- **Receiver start = viewport, not delivery.** wire-webapp starts the
  countdown when the message scrolls INTO THE VIEWPORT (actually
  viewed), persisting `ephemeral_started` / `ephemeral_expires` then.
  The **sender** starts the timer on its own copy as soon as the send
  hits the backend. A headless SDK has no viewport, so Kupferdraht's V1
  starts the receive-side timer at dispatch — the safe direction
  (never later than an official client; may delete sooner). For the
  OTP use case this is good news on the official-client side: the
  code does not expire unseen.
- **Expiry enforcement is asymmetric.** On expiry the official
  *receiver* sends a delete-for-everyone (`MessageDelete`, targeted
  to self + sender in groups, untargeted in 1:1; local-only when self
  was removed / self-ping); the *sender* only obfuscates its local
  copy and waits for that delete. Kalium's protocol behaviour matches
  (read 2026-09-16 as observation only): as receiver it marks the row
  deleted, sends a `DeleteForMe` into its self-conversation for its
  own other devices, waits for sync-live, sends a `MessageDelete`
  addressed to the original sender, then purges; as sender it only
  marks the row and relies on the receiver's delete. Kupferdraht mirrors
  the receiver's `MessageDelete` (decided by Volker 2026-09-16 —
  "if we build it, build it completely") and the sender's local-only
  handling; the `DeleteForMe` needs the self-conversation (Step 86).
  Under MLS the "addressed to the sender" refinement is moot — an
  application message is encrypted to the whole group.
- **Official clients bound the timer** to roughly 1 s … 1 year;
  Kupferdraht's send-side validation uses the same range
  (`timeline/ephemeral.rs`).
- **Traffic of the receiver broadcast:** one MLS application message
  per expired inbound ephemeral per receiving device — the same order
  as a read receipt. Irrelevant for the OTP driver (Kupferdraht is the
  sender there; the webapp receiver sends the delete, Kupferdraht's
  inbound `MessageDelete` path tombstones).

Live smoke against schwarz-dev-wire (2026-09-15) covers the scripted
kupferdraht→kupferdraht round-trip and kupferdraht→webapp delivery; the webapp
countdown rendering and the webapp→kupferdraht direction with a typed
timer are the eyes-on checklist in the Step-89 report.

---

## 75) SQLite3MC 2.5.1 re-examined: the §52 hang is gone, the header pragma works, the pragma order is inverse to SQLCipher's, and ChaCha20 is not a BSI cipher (2026-09-15)

Step 88 (`docs/plan/archive/88-sqlite3mc-verification.md`, harness in
`spike/sqlite3mc/`) re-ran the two findings that pushed Kupferdraht off
SQLite3MC in June (§52 hang, ADR-0013 driver 1) against the current
release, plus a paper check of the cipher schemes. Five things worth
keeping regardless of the engine decision:

- **The §52 `PRAGMA key` hang does not reproduce on 2.5.1** — not with
  the sqlcipher scheme selected per connection, not with the exact §52
  build (`-DCODEC_TYPE=CODEC_TYPE_SQLCIPHER`, no cipher pragma), not in
  the debug profile (`cc` at `-O0`): ~0.5–2 ms per `PRAGMA key`. Even
  the passphrase path (256 000 PBKDF2-SHA512 iterations at `-O0`) is
  ~250 ms, so the June "unoptimised software AES/KDF" hypothesis does
  not explain a > 60 s stall either. The 2.3.5 root cause stays unknown;
  it is simply not there any more. Lesson: an unreproduced hang recorded
  as an engine property cost a vendored OpenSSL.
- **`plaintext_header_size` was a real upstream bug, not our build**:
  utelle/SQLite3MultipleCiphers #233 — the sqlcipher scheme applied the
  parameter only for `legacy >= 4` and ignored it in non-legacy mode
  (`legacy = 0`, the default); chacha20/ascon/aegis were unaffected.
  Fixed in 2.3.4 (2026-05-06). ADR-0013's "provably does not engage" was
  observed on 2.3.5's predecessor path with the default `legacy = 0`.
- **SQLite3MC's pragma contract is the inverse of SQLCipher's.** Cipher
  parameters (`cipher`, `legacy`, `plaintext_header_size`) and
  `cipher_salt` (bare 32 hex digits, not `x'…'`) must be set BEFORE
  `PRAGMA key`; afterwards `cipher_salt` is read-only ("Cipher salt
  can't be changed"). Kupferdraht's production batch (`key` →
  `cipher_plaintext_header_size` → `cipher_salt = "x'…'"`) is rejected
  outright. Equivalent shortcut: the 96-hex raw form
  `PRAGMA key = "x'<key><salt>'"`. There is no `PRAGMA cipher_version`;
  engine identity is `SELECT sqlite3mc_version()`.
- **2.5.1's amalgamation hooks `sqlite3mc_initialize` into
  `sqlite3_initialize` itself.** The `-DSQLITE_EXTRA_INIT=
  sqlite3mc_initialize` flag §51 needed for 2.3.5 must NOT be passed any
  more — a second registration would fail initialisation, not no-op.
  Pin the engine at runtime (`sqlite3mc_version()` + an encrypted-header
  control) instead of trusting a compile flag.
- **Salt semantics differ per scheme with a raw key.** sqlcipher derives
  the page-HMAC subkey from the salt, so right key + wrong salt fails
  (Kupferdraht's `right_key_wrong_salt` pin holds). chacha20 (sqleet format)
  runs no KDF for a raw key: the salt is inert and the same probe opens
  the file. And SQLite3MC's sqlcipher scheme is byte-compatible with
  SQLCipher 4 including our plaintext-header + external-salt layout: the
  Step-76 `keyed.db` fixture opens in both non-legacy and `legacy = 4`
  mode with `integrity_check` ok.
- **BSI TR-02102-1 (2026-01) lists AES only**; §3.2 recommends no
  dedicated stream cipher, and ChaCha20/Poly1305/AEGIS/Ascon do not
  appear in TR-02102-1 or TR-02102-2 at all. §52's "ChaCha20 is BSI
  TR-02102-conformant" was a wrong claim in the record; the sqlcipher
  scheme (AES-256-CBC, random per-page IV, HMAC-SHA512,
  Encrypt-then-MAC) is the one whose every primitive is on the lists.

---

## 76) `login()` touches the OS keystore before it validates the server URL — a Keychain-less sandbox turns `http_hardening` into a 30-minute hang (2026-09-15)

Running the Step-88 gate inside a sandboxed session (no interactive
macOS Keychain access), `kupferdraht-core/tests/http_hardening.rs::
login_rejects_plain_http_url_without_permit_http` hung for ~30 min and
then failed with a non-`InvalidInput` error, while CI on the same base
commit (`f35967b`) is green. Cause, from `client/builder.rs`: `login()`
calls `resolve_keystore(…, permit_http = false)` — the production
`KeyringStore` — and then `install_cookie_label(&keystore)` (a real
Keychain read/write) BEFORE `HttpClient::new` performs the `https://`
check the test is about. Where the Keychain ACL cannot be satisfied
(unsigned binary, no UI session), the Security framework blocks until
its own timeout; the test's expected rejection never gets a chance.
`KUPFERDRAHT_DEV_KEYSTORE=<dir>` (the file-keystore escape hatch
`default_keystore` documents for exactly this macOS case) makes all
three tests pass in 10 s. Two consequences: the gate recipe on a
sandboxed Mac needs that variable for this target (CI on Linux is
unaffected); and the order in `login()` is a small design smell — the
cheapest, side-effect-free check (URL scheme) should run first, before
any keystore I/O (open.md gap-B.26).

---

## ~~77) libsqlite3-sys's non-bundled route still probes pkg-config before honouring `SQLITE3_LIB_DIR` — and GitHub's Ubuntu runners ship `pkg-config` + `libsqlite3-dev` (2026-09-16)~~

*Closed 2026-09-24 — moot since Step 90: Kupferdraht links SQLite3MC through the bundled route (vendored amalgamation), never `SQLITE3_LIB_DIR`. Text kept unchanged as the findings log.*

Re-checked for Step 88 P4 against the I1 template (the sister project links
SQLite3MC through libsqlite3-sys's non-bundled path: `SQLITE3_STATIC=1`
+ `SQLITE3_LIB_DIR`, the amalgamation compiled by the crate's own
`build.rs`). The §51 finding from June is unchanged in libsqlite3-sys
0.35.0 AND 0.38.1 (identical `find_sqlite`): with `SQLITE3_LIB_DIR`
set, the build script sets `PKG_CONFIG_PATH=<dir>/pkgconfig`, probes
pkg-config for `sqlite3 >= 3.14.0`, and emits its own
`rustc-link-lib=static=sqlite3` + search path **only if that probe
fails**. It works on a Mac without `pkg-config` (this one) and silently
links the system SQLite wherever a `sqlite3.pc` is findable — GitHub's
ubuntu-24.04 image lists `pkg-config 1.8.1` and `libsqlite3-dev 3.45.1`,
so a Linux CI job would get an unencrypted database with no build error.
For Kupferdraht this closes the mechanism question: `openmls_sqlite_storage`
0.3 forces `bundled` anyway, and a `[patch.crates-io]` stand-in makes
`bundled` compile the SQLite3MC amalgamation deterministically, with no
environment probe in the path. Whichever route a project takes, an
engine-identity pin (`SELECT sqlite3mc_version()`) plus an at-rest
ciphertext check is what turns "linked the wrong SQLite" into a red
test instead of a plaintext file.

## ~~78) refinery's default runner commits a migration and its history row separately — and `openmls_sqlite_storage` 0.3.0 inherits the window (2026-09-24)~~

*Closed 2026-09-24 — resolved for kupferdraht.db by Step 87 (grouped runner); mls.db residual tracked as open.md gap-B.27 (upstream issue draft in the archived Step-87 doc). Text kept unchanged as the findings log.*

Step 87 fix + verification. refinery-core 0.9.1's rusqlite driver
opens one transaction per `execute` (`src/drivers/rusqlite.rs:33-47`);
non-grouped `migrate` issues each migration as two executes — SQL,
then the history insert (`src/traits/sync.rs:85-99`, the `i % 2`
loop). A kill between the two commits leaves the migration applied
but unrecorded; the next open re-applies it. Classified on the
current tree: **25 of 27** kupferdraht.db migrations are re-apply-fatal
(only V17 and V18 are fully `IF NOT EXISTS`; V27 from Step 89 joined
the fatal set). `set_grouped(true)` puts every pending migration plus
its history rows in one transaction; SQLite DDL is transactional, so
a failure anywhere rolls the whole run back — pinned by fault
injection in `storage/schema.rs` (V2 pre-created → V1 must not
survive). Two traps: (1) `set_grouped(self, bool) -> Runner` takes
`self` by value while `set_migration_table_name` takes `&mut self` —
the grouped call must stay in the builder chain; (2) the history
table is created by refinery in its own preceding execute, so the
post-failure assertion is "exists but empty", not "absent". Upstream:
`openmls/openmls` `main` `sqlite_storage/src/storage_provider.rs`
still runs non-grouped (`run_migrations` sets only the table name),
newest crates.io release 0.3.0 (2026-08-25), no matching issue — the
draft is in `docs/plan/archive/87-atomic-migrations.md`; mls.db keeps the
window until it ships (open.md gap-B.27).

## 79) SQLite3MC in the workspace, three gotchas the swap surfaced: pragma read-back is TEXT, the release signature is Sigstore keyless, a `[patch]` path inside the repo is not a workspace member (2026-09-24)

Step 90 (`docs/plan/archive/90-sqlite3mc-engine-swap.md`) executed ADR-0021 on
the Step-88 evidence; the mechanics held (patch stand-in, no
`SQLITE_EXTRA_INIT`, pragma order parameters-before-key, first-read key
verification). Three things the harness had not touched:

- **SQLite3MC answers its cipher-parameter pragmas as TEXT.** `PRAGMA
  plaintext_header_size` on a keyed connection returns the column
  `"32"` as text, not an integer — rusqlite's `row.get::<_, u32>`
  fails with `Invalid column type Text`. Read the value as `String` and
  parse it (runtime data, a recoverable `KupferdrahtError::Storage`). `PRAGMA
  cipher` is text by nature (`chacha20`); `SELECT sqlite3mc_version()`
  is text (`SQLite3 Multiple Ciphers 2.5.1`).
- **The release's checksum signature is Sigstore keyless, not GPG.** The
  v2.5.1 assets are `sqlite3mc-2.5.1-SHA256SUMS`, `….sig` (base64 of a
  DER ECDSA signature, 71 bytes decoded) and `….pem` (base64 of a PEM
  certificate — double-encoded). Without `cosign`, `openssl dgst -sha256
  -verify <pubkey from the .pem> -signature <decoded .sig> SHA256SUMS`
  verifies the signature against the embedded Fulcio certificate (issuer
  `sigstore-intermediate`, SAN = the utelle GitHub-Actions workflow URI,
  10-minute validity around the publish time). What that does NOT check:
  the chain to the Fulcio root and the Rekor inclusion — `cosign
  verify-blob` does; `vendor/sqlite3mc/PROVENANCE.txt` records exactly
  which half was verified.
- **A `[patch.crates-io]` path crate inside the repository directory does
  not become an implicit workspace member.** `cargo metadata` lists the
  three real members only, so `cargo fmt --all` / `clippy --workspace`
  do not lint the vendored stand-in (upstream code, `unsafe` and all) and
  no `exclude` is needed; the gate stays a pure Kupferdraht-code gate while the
  stand-in still builds as a dependency. The Step-76 fixture `keyed.db`
  converted in place exactly as harness bonus 2 predicted (sqlcipher
  scheme → `PRAGMA cipher = 'chacha20'; PRAGMA rekey`), header byte 20
  going 0x50 → 0x20, `integrity_check` ok, journal mode `delete`, no
  sidecars — the guarded 0.8.1 generator was not needed.

---

## 80) SQLite3MC in the browser: five things the OPFS spike settled that the docs of the crates do not say (2026-09-24)

Step 91 (`docs/plan/archive/91-sqlite3mc-wasm.md`, ADR-0022) put the
native `SqliteStore` on `wasm32-unknown-unknown` over the OPFS `sahpool`
VFS. Verified in headless Firefox and Chrome 153, dedicated worker
(`spike/sqlite3mc-wasm/` — retired at archive time, last at `6bb5277` —
then `kupferdraht-core/tests/step_91_*.rs`):

- **`sqlite-wasm-rs` 0.6.1 bundles the byte-identical SQLite3MC 2.5.1
  amalgamation** Kupferdraht vendors (SHA-256 equal to `PROVENANCE.txt`) and
  honours `SQLITE_WASM_RS_SOURCE_DIR` — so the vendored file is the
  engine on every target by configuration, not by coincidence. 0.5.5
  bundled 2.3.3; `sqlite-wasm-vfs` 0.3 dropped the `relaxed-idb` VFS
  (only `sahpool` remains, behind a feature).
- **rusqlite 0.37 has no browser path** (that arrived in 0.38), and the
  0.37 line is pinned by `openmls_sqlite_storage` 0.3 (refinery 0.9.2 caps
  rusqlite at 0.39). A wasm arm in the `libsqlite3-sys` stand-in
  (`pub use sqlite_wasm_rs::*` plus two stubs for the deprecated
  `sqlite3_trace` / `sqlite3_profile` the crate compiles out) is all it
  takes — rusqlite, refinery and openmls_sqlite_storage then compile and
  run unchanged. refinery's `applied_on` stamp needs `time`'s
  `wasm-bindgen` feature, or `SystemTime::now()` panics inside the first
  migration.
- **`sahpool` has no shared-memory WAL.** `PRAGMA journal_mode = WAL`
  silently stays `delete`; under `locking_mode = EXCLUSIVE` it is
  accepted — but then EVERY later open must set exclusive locking before
  its first read, or the WAL-header file (bytes 18/19 = 2/2) fails with
  `SQLITE_CANTOPEN`. Kupferdraht keeps the rollback journal on wasm; a
  natively written fixture must be closed in `DELETE` mode before the
  browser can open it. The VFS also implements no locking at all: one
  connection per file, no cross-tab coordination.
- **SQLite3MC wraps only the VFS that was default at init** (the in-memory
  one). The pool must be wrapped explicitly (`sqlite3mc_vfs_create`, the
  `multipleciphers-opfs-sahpool` shim) or `PRAGMA key` fails with
  "Encryption is not supported by the VFS" (the sister project's bugfix, adopted).
  The one FFI call lives in the vendored stand-in behind a safe wrapper.
- **The headless harness cannot prove reload survival**: every
  wasm-bindgen-test run is a fresh browser profile, so OPFS state never
  survives between runs. Persistence across connections and the at-rest
  bytes are pinned (the pool's `export_db`); reload survival needs a
  persistent profile or a manual browser smoke. Chrome needs a
  chromedriver matching the installed major (wasm-pack downloads the
  latest — 154 against Chrome 153 here); geckodriver is version-tolerant,
  which is why the script and CI use Firefox. Apple's clang has no
  WebAssembly backend — Homebrew LLVM (`CC_wasm32_unknown_unknown`) is
  needed for the C compile; distro and GitHub-runner clang have it.

---

## 81) Web Locks from wasm: the lease is one extern block, a lock is not reentrant within an agent, and `ifAvailable` and `steal` exclude each other (2026-09-24)

Step 92 (`docs/plan/archive/92-single-active-tab.md`, ADR-0023) put the
one-instance-per-origin lease on the Web Locks API. What the spec pages
do not spell out for a Rust caller:

- **No `web_sys_unstable_apis` needed.** `web-sys` 0.3.105 keeps
  `LockManager` behind that cfg, but a hand-written
  `#[wasm_bindgen(js_namespace = ["navigator", "locks"], js_name = request, catch)]`
  extern block binds `navigator.locks.request` in both `Window` and worker
  scopes — nested `js_namespace` arrays work, and `catch` turns a browser
  without `navigator.locks` into an `Err` instead of a thrown TypeError.
- **Keeping a lock = a callback that returns a never-settling promise.**
  The lock lives exactly as long as that promise (or the agent); the
  outer `request()` promise stays pending meanwhile and **rejects with
  `AbortError` when another context steals the lock** — verified in
  Firefox and Chrome 153; that rejection is the eviction signal, awaited
  in a `spawn_local` task. Whether the grant happened is learned from the
  callback's argument (`null` under `ifAvailable`), handed back through a
  `tokio::sync::oneshot` — the callback runs on a later microtask, never
  synchronously.
- **`ifAvailable: true` together with `steal: true` is a
  `NotSupportedError`** — build the options per intent (normal open:
  `ifAvailable`; takeover: `steal`), never both.
- **A lock is not reentrant within one agent**, so a test can play
  "another tab" in the same worker: a second raw request from the test
  holds the lease before Kupferdraht installs, is refused/evicted exactly like
  a foreign tab, and a probe request with `ifAvailable` answers `null`
  while anyone holds it. The scripted two-tab smoke plays both tabs from
  one page for the same reason (Web Locks are shared across every
  same-origin context) — the headless harness cannot open two real tabs.
## 82) The self conversation is listed at epoch 0 but has no GroupInfo — the first device must establish it, external commits are impossible (2026-09-24)

The Step-86 orientation probe against the own instance
(schwarz-dev-wire, test user 3, fresh device) settled the open
protocol question from `docs/plan/archive/86-self-conversation.md`:

- `sync` lists the MLS self conversation in the rooms snapshot right
  away: `type: self_room`, `epoch: 0`, a `group_id`, one member. It is
  not hidden while unestablished (the 1:1 reservation is, §66/§67).
- `room-join` against it → `404 mls-missing-group-info` ("The
  conversation has no group information") on `GET …/groupinfo`. That
  is the same failure the 2026-08-24 `rooms-join-all` re-probe logged
  for "a conversation that was never MLS-established" (§70) — it was
  this room. Wire accepts **no** external commit into a
  never-established group: there is no `GroupInfo` to build one from.
- So the design follows the Step-31/68 precedent, not a new path:
  at `epoch == 0` the client creates the local group under the listed
  `group_id` and runs the establishing commit (self-claim the user's
  other devices → Add-commit, or a self-update commit when alone);
  Wire accepts the bundle and the room is at epoch 1. Every later
  device of the same user external-joins as usual (epoch > 0). The
  branch lives in `Client::join_room` (self room ∧ epoch 0), so
  `join_all_rooms`, `Room::join`, `room-join` and `send_text_auto_join`
  all heal it; `Client::self_room` is the lookup. A rejected
  establishing bundle discards the local group (Step-66 pattern) so the
  next `join_all_rooms` retries instead of reporting a dead
  `already_joined`.
- Not an RFC 9420 deviation (`mls-deviations.md` "What's not a
  deviation"): the protocol has nothing to publish before the group
  exists either. It is Wire's listing model — the conversation envelope
  precedes the MLS group. `GET /v14/conversations/mls-self` exists in
  the swagger but is not needed: `list-ids` already carries the room.

Side findings of the same session, resolved the same day:

- **Pre-Step-90 smoke homes were dead for the current binary, and the
  shared dev keystore was the tripwire.** Every `~/.kupferdraht-smoke*` home
  written before the engine swap failed at `status`/`login` with `DB key
  material must be 32 bytes (raw key), got 48` — not at the database,
  but at the 48-byte legacy blob the shared `FileKeyStore` directory
  still held for each account (I3, hard 32). Fix applied 2026-09-24:
  the old keystore and all 17 pre-swap homes are parked under
  `~/.kupferdraht-smoke-pre-step90/`, a post-swap keystore took the shared
  path, the per-step scripts run on the three persistent env homes with
  a `FRESH=1` gate (LEARNINGS §5.20/§5.32).
- **The dead homes' devices still held every test user at the 7-device
  cap** (user 3 hit 7/7 with the one fresh login this step needed).
  Reset the same day by Volker's decision ("delete everything, start
  over"): every registered device of the three test users deleted, one
  persistent device per user re-registered (Wire rate-limits bursts of
  `DELETE /clients/{id}` with `429 too-many-requests` — pace the loop).
  The two-device smoke then ran green: device B external-joined the
  self room at epoch 2 and read a task device A sent after B's join.

## 83) One refresh cookie per `(user, label)` — two homes on one dev keystore share the install id, and the later login evicts the earlier home's session (2026-09-24)

Found while re-checking the smoke homes after the Step-86 merge of
`main`: `status` on user 3's home 3 answered `session expired; the
refresh token is no longer valid` while homes 1, 2 and user 3's second
home 3b were fine. Not the Step-77 keystore change (file names and
format unchanged) — Wire's cookie model:

- Kupferdraht's login label is `kupferdraht-<install-id>`; the install id is
  generated once and persisted **in the keystore** (`kupferdraht.install-id`,
  Step 41, `client/builder.rs::install_cookie_label`), precisely so that
  "two devices of one account never collide on Wire's `(user, label)`
  cookie slot and invalidate each other's refresh `zuid`".
- Every home that shares a keystore therefore shares the label: all
  homes on one `KUPFERDRAHT_DEV_KEYSTORE` directory (the smoke setup), or two
  consumers on one macOS user account with the keychain service
  `kupferdraht` (CLI + TUI). For two homes of the **same** Wire account the
  Step-41 guarantee is void.
- Wire keeps one refresh cookie per `(user, label)`: a login with an
  already-used label replaces the earlier cookie. The earlier home
  keeps working until its 15-minute access token expires, then its
  `POST /access` fails and Kupferdraht surfaces `SessionExpired`.

Live timeline (test user 3, shared dev keystore): home 3b logged in
15:52 → home 3 still worked at 15:53 (valid access token) → dead at
17:17. Re-login of home 3 at 17:19 (device reuse) → home 3b dead at
its next `/access` after the token expiry, home 3 alive. Both
directions, deterministic. Different accounts on one keystore are
unaffected (the slot is per user).

Consequences: the Step-86 smoke script gives device B its own keystore
directory (`<home>.keystore`) and stage C restores the sessions; a
two-home smoke of one user on a shared dev keystore is a trap even
with the device-reuse pattern (LEARNINGS §5.20). Production-relevant
for two Kupferdraht consumers on one desktop user signed into the same
Wire account — open.md gap-B.33: derive the label from a per-home id
(persisted in the home's `global.db`) rather than the keystore-wide
install id.


## 84) A resend under the original `message_id` is delivered once and lands in resend order — Wire has no memory of the failed attempt (2026-09-24)

Step 79 smoke on the own instance: three sends of one device (text,
text, ephemeral 120 s) failed behind a dead HTTPS proxy after local
MLS encryption, then went out again under their persisted
`message_id`s — one via `resend --message`, two via `resend --all`.
Wire accepted all three (`201`), the receiver's next drain carried
exactly three events, each id appears once in its timeline, and the
ephemeral arrived with its timer (`⏱ until`, 120 s from the resend
instant, not from the failed attempt). Two consequences the API
relies on: (a) the failed attempt's ciphertext — a ratchet generation
the peer never saw — is skipped without complaint (openmls forward
tolerance), so a resend never needs an epoch change; (b) keeping the
id is safe against the "5xx from a proxy after the backend stored it"
case, because a receiver that already holds the id keeps its row
(the receive path logs the `UNIQUE constraint` hit and skips the
duplicate) instead of showing the message twice. The sender's own timeline drained nothing (no self-echo
for the sending device, unchanged), so its rows flip to `Submitted`
and stay there.

## 87) A browser login against Wire needs a same-origin proxy, and the proxy must rewrite the `zuid` cookie's Path (2026-09-25)

The first live browser login vertical (Step 94 smoke, headless Firefox,
two real tabs) hit three things in a row that a consumer will hit too:

- **CORS:** a page on `http://127.0.0.1:<port>` cannot call the Wire
  instance directly — `GET /api-version` fails in the worker as reqwest's
  "error sending request" (the browser's CORS refusal, not a network
  error). `docs/bindings.md`'s "same-origin dev proxy" is a requirement,
  not a convenience.
- **The proxy must sit at the origin root:** `HttpClient::url` joins
  request paths with `Url::join`, which replaces a base path — a base URL
  `http://127.0.0.1:8094/wire` sends `GET /api-version`, not
  `/wire/api-version`. Route "every path that is not a page file" to
  Wire.
- **The refresh cookie:** Wire sets `zuid` with `Path=/access; HttpOnly;
  Secure` (no `Domain`, no `SameSite` on this instance). Through a proxy
  the versioned refresh `POST /v14/access` does **not** match
  `Path=/access`, so the browser never sends the cookie and the first
  refresh fails `SessionExpired` right after `registerDevice`. Rewrite to
  `Path=/` (and drop any `Domain`); keep `Secure` — `127.0.0.1` is a
  secure context in Firefox and Chrome, and dropping it would break a
  `SameSite=None` cookie elsewhere. With that, login → registerDevice →
  syncOnce → sendText → messages → `onSync` ran end to end.
- **Cold cache after `resume`:** the promoted leader read `accounts()`
  from the OPFS `global.db` the first worker wrote, but `rooms()` was
  `[]` until its first `syncOnce` — the wasm `rooms()`/`messages()` read
  the in-memory cache (open.md gap-B.35), not the store.

## 86) Leader election on the Web Locks queue needs a hand-off: a queued JS grant cannot be re-taken by `Kupferdraht.open`, it must be stolen from it (2026-09-25)

Step 94 (`docs/plan/archive/94-leader-tab-thin-clients.md`, ADR-0026) put the
multi-tab election on the queue of the ADR-0023 lease. What the plan's
"queue in JS, then `Kupferdraht.open` on the grant" hides, and what the
orientation verified against the W3C Web Locks algorithm + headless
Firefox:

- **A lock is not reentrant within an agent** (§81), so the candidate
  worker whose JavaScript callback holds `kupferdraht:instance` cannot pass
  `Kupferdraht.open`'s own `ifAvailable` request — that open fails with
  "already open in another tab" against its *own* grant. Releasing first
  (returning from the callback) hands the lock to the **next queued tab**
  before the Rust request even exists: FIFO grant, no gap for a
  re-take. The race-free hand-off is `Kupferdraht.open({ takeOver: true })`
  **inside the grant callback**: `steal` releases the held lock (the
  candidate's own request rejects with the expected `AbortError`) and
  *prepends* the stealing request, so nothing queued can slip in. The
  takeover's five-second install retry also covers a just-terminated
  leader's lingering OPFS handles. Zero change to the Rust lease.
- **Agent termination is the whole crash story:** the spec releases a
  terminated agent's held locks and aborts its queued requests, and every
  dedicated worker is its own agent — `worker.terminate()` on the leader
  promotes the next queued worker with no heartbeat involved (the
  headless test pins it). A heartbeat is a UI hint only.
- **A tab re-elected after its own eviction must announce a higher
  epoch:** the evicted worker is terminated, so the *host* carries the
  highest epoch seen into the fresh candidate's `init` message.
- **wasm-bindgen local snippets may live outside the crate:**
  `#[wasm_bindgen(module = "/../bindings/web/kupferdraht-tabs/host.js")]` reads
  the file relative to `CARGO_MANIFEST_DIR` and writes it to
  `snippets/<crate>-<hash>/../bindings/…` = `snippets/bindings/…` beside
  the test glue, directory structure preserved, so relative imports
  between the copied files resolve and a test-only driver under
  `tests/web/` imports the module as `../../../bindings/web/kupferdraht-tabs/`.
  The runner's static server would not serve it otherwise (`rouille`
  canonicalises and refuses paths outside the cwd, symlinks included).
  Bind a JS `const` as `#[wasm_bindgen(thread_local_v2)] static`, not as
  a function ("… is not a function" at call time).
- **The effective Safari floor has been 15.4 since ADR-0023**, not the
  15.2 the OPFS floor names — Web Locks and `BroadcastChannel` both need
  15.4; nothing in Step 94 raises it further.
- **`BroadcastChannel` fan-out cost is per listener:** a single shared
  channel would clone every response into every tab; a per-tab response
  channel (`kupferdraht:tabs/<tabId>`) keeps responses at one copy per hop.

## 85) rusqlite already sets a 5-second busy timeout on every connection — open.md §69's "no `busy_timeout`" residual never existed as stated (2026-09-24)

Step 85's red tests for the §69 residual came up **green before the
implementation**: `PRAGMA busy_timeout` read back `5000` on `kupferdraht.db`,
`global.db` and `mls.db`, and a second connection's write waited out a
300 ms `BEGIN IMMEDIATE` writer instead of failing `database is
locked`. Source: `rusqlite` 0.37 calls `sqlite3_busy_timeout(db, 5000)`
in `InnerConnection::open_with_flags` (`src/inner_connection.rs`) for
every connection it opens. The §69 grep for the pragma in Kupferdraht's
source was correct and the conclusion drawn from it was not — the
behaviour came from a dependency default. Step 85 pins it explicitly
at the three open paths (`storage::set_multi_process_pragmas`:
`busy_timeout = 5000`, `synchronous = FULL`) so the multi-process
contract does not hinge on a crate default, and the tests stay as
regression pins. Lesson for reviews: "no pragma in our code" is not
"not in effect" — read it back off the live connection (LEARNINGS
§5.28) before recording a residual.

Two harness facts from the same step, worth keeping: `flock` (the
standard library's `File::try_lock` on Unix) binds to the open file
description, so two `File` handles in one process contend exactly like
two processes — an in-process two-thread test is an honest lock test;
and libtest running single-threaded (`--test-threads=1`) prints
`test <name> ... ` **without a newline** before the test's own stdout,
so a parent process scanning a child test's output for a marker line
must match `contains`, not equality (the first cross-process harness
run hung on exactly this).


## 86) Two homes of one user on ONE dev keystore keep both sessions across a `/access` each — the §83 eviction is gone; and a rebind does not always rotate the `zuid` (2026-09-25)

The Step-93 smoke replayed §83 on the shared `KUPFERDRAHT_DEV_KEYSTORE`
with test user 3 — the persistent home 3 (keyed before Step 93:
keystore-wide ids, no `home.id`) and a freshly logged-in home 3b:

- 13:34Z home 3b logs in (fresh home, new device; own id `4ced…`, own
  label `kupferdraht-4ced…`, and a **fresh** account key — the keystore
  still holds home 3's `account-<u3>` legacy entry, but a database that
  does not exist yet is never adopted). Then `status` on home 3: its
  three keys are adopted byte-for-byte under `kupferdraht.5adb….*` with the
  legacy entries untouched (SHA-256 compared), and its `/access` on the
  516-minute-old token succeeds on the **old** shared-label cookie —
  before Step 93 exactly this call died `SessionExpired`, because 3b's
  login had replaced the cookie under the shared label. `status` shows
  `session_label: —` for the pre-Step-93 session; a re-login (device
  reuse, no slot) records `kupferdraht-5adb…`.
- 13:51Z, both tokens 16 min old: `status` on 3b and on home 3 → one
  `/access` each on its own cookie, both succeed, no `SessionExpired`.
  Before Step 93, home 3's re-login would have killed 3b here
  (direction 2 of §83).

An adoption dry run on a **copy** of home 3 (`accounts` only — no
resume, so no `/access` on the cookie the real home was about to use)
adopted the registry key under a third id; the copy and its stray key
were removed afterwards. Keystore after the smoke: the seven legacy
entries plus `kupferdraht.install-id` (kept on purpose, ADR-0025 H3), and
one home-scoped triple per home.

Side finding, not investigated: both rebinds answered **without** a
`set-cookie` (`/access response — zuid rotation analysis rotated=false`
in the debug log — the flag is "the response carried a `zuid`"). Wire
does not rotate the refresh cookie on every `/access`; the Step-85
smoke saw rotations on the same instance. Hypothesis: brig renews a
persistent cookie only past `setUserCookieRenewAge` — unverified.
ADR-0024's critical section is unaffected (it guards the rotation
whenever it happens), but its Context sentence "Wire invalidates the
`zuid` on every successful `POST /access`" reads as "when it rotates".

## 88) An inbound MLS message's envelope names the user, not the device — the device id comes from the signer's leaf, and a real Wire credential's `{client_id}` is the API's client id (2026-09-26)

Step 95a live smoke against schwarz-dev-wire (ADR-0027 — inbound
messages are attributed to the MLS-authenticated leaf, the envelope is
a cross-check):

- **`conversation.mls-message-add` carries no `from_client`.** The
  event is `qualified_conversation`, `qualified_from`, `data`, `time`,
  `via` (§68 on the `/await` frame, same shape on the pull
  transport). Before Step 95a every inbound MLS message therefore had
  an empty `sender_device_id` (`Message::sender_device_id == None`,
  `TimelineItem::sender_device_id == ""`); since 95a it is the
  `{client_id}` segment of the signer's W-4 credential. Consequence
  for the ADR-0027 cross-check: on Wire it compares the **user** only
  — the device half is dormant unless a backend starts sending
  `from_client`.
- **Kupferdraht → Kupferdraht** (user 2's fresh device → user 1's fresh device,
  their 1:1, both joined by external commit): user 1 attributes the
  message to user 2; no mismatch or malformed-credential warn across
  the join, the 6-notification drain and the message.
- **Official webapp → Kupferdraht** (user 2 on the webapp → user 1): the
  message is attributed to user 2, no mismatch warn, no X.509
  fallback — a leaf credential built by an official client parses as
  W-4 (`{uuid}:{client_id}@{domain}`, three non-empty parts) and names
  the same user the envelope claims.
- Not observed in the smoke: the webapp device's `{client_id}` against
  its API client id (the CLI prints no device ids and the envelope has
  none to compare). Official clients build the credential's
  `{client_id}` from the API client id string, so the two are the same
  value; the unit tests pin that Kupferdraht keeps the segment verbatim.

## 89) The MLS thumbprint Kupferdraht computes equals the one the official webapp shows — and device changes surface in every shared room (2026-09-26)

Step 95d live smoke against schwarz-dev-wire:

- **Same thumbprint across clients.** For user 1's Kupferdraht device
  `<client-id>`, `kupferdraht-cli status` / `room-devices` print
  `Gd Fk HR kq p5 tL iG x0 rv EV 6f 65 N_ Q5 Cq ZR mp rB pq a7 -s Q`; the
  official webapp (logged in as user 2, user 1's profile → devices) shows
  "MLS-Daumenabdruck" (MLS fingerprint) `GdFkHRkqp5tLiGx0rvEV6f65N_Q5CqZRmprBpqa7-sQ` — the
  same string (RFC 7638 JWK thumbprint of the P-256 signature key,
  SHA-256, base64url without padding). The webapp computes it from the
  leaf Kupferdraht created, so the two sides agree on encoding and key.
- **Own view = peers' view.** A fresh second device of user 2 printed
  its own thumbprint in `status`; user 1's `sync` reported the device's
  arrival with the identical value.
- **Changes surface per shared room.** Removing that device
  (`logout --purge-device`) and adding a new one (`login` +
  `rooms-join-all`) each produced one `devices changed` report in
  **both** rooms user 1 shares with user 2 — the device is a leaf in
  each group separately.
- **A device id need not be 16 characters.** The new device got
  `6ce757f92d60d12` (15 hex digits — no zero padding); the credential's
  `{client_id}` and the API id are the same string (cf. §88).
- **The first epoch change of a pre-existing room is not a baseline.**
  Before the fix in 95d a room whose first observed epoch change was the
  new device recorded that device silently; `sync_once` now records every
  room's baseline before it drains (see `docs/library.md` "Room devices").

## 90) Foreign data that used to panic — what each layer accepts without complaint (2026-09-26)

Step 95c inventoried every `assert!` / `debug_assert!` in library code by
where its operand comes from: 132 of about 280 guarded runtime data.
Findings worth keeping:

- **prost does not enforce proto2 `required`.** A `TaskPayload` without
  `task_id` or a `CasePayload` without `case_ref` decodes to `""`. Kupferdraht
  treats such payloads as absent (a task message then shows its `Text`
  fallback). Before 95c an empty `task_id` killed the sync task in
  release builds too, *after* the timeline row was written — databases
  from before 95c can hold such rows; the task fold skips them.
- **openmls accepts an empty group id in a Welcome.** The id sits inside
  the encrypted `GroupInfo`, so the inviter chooses it and the backend
  cannot check it. `absorb_welcome` refuses it as `Encoding`; the old
  `assert!` fired while holding the MLS mutex and poisoned it for the
  rest of the session.
- **Peer and backend strings end up in URL paths.** `Asset.RemoteData`
  (`asset_domain`, `asset_id`) and `user.properties-set` keys were
  formatted straight into the bearer-authenticated GET path — `../self`
  or `%2e%2e` (the URL parser normalises percent-encoded dot segments)
  sent the request to another backend path. Such segments now pass only
  as RFC 3986 unreserved characters (`A-Z a-z 0-9 - . _ ~`), not `.`/`..`.
- **A peer's `expire_after_millis` is an `int64`.** `i64::MAX` overflowed
  the expiry stamp; inbound timers are capped at Kupferdraht's send-side
  maximum (1 year). A case admin's `revision: u64::MAX` overflowed the
  next local `set_case`; that update is now refused.
- **`POST /conversations/list` takes at most 1000 ids** — see
  `docs/wire/api-notes.md`.
- **wasm cannot catch a panic** — it traps the module; native FFI sync
  loops now report one via `on_error`.

## 91) The official webapp ignores a self-sent delete of another member's ordinary message (2026-09-28)

Black-box test against schwarz-dev-wire, to ground ADR-0028's clause
(b) in observed behaviour (review question on PR #56):

- A second device of user 2 ran a pre-95b `kupferdraht-cli` (main
  `e85b438`, which has no sending-side guard) and sent a
  delete-for-everyone (`MessageDelete`) of a message user 1 had just
  sent to the shared room "Test". A second message of user 1, sent at
  the same time, was left alone as the control.
- **The official webapp, logged in as user 2, kept showing both
  messages.** It had decrypted the messages before and after the delete
  in the same epoch, so the delete reached it — it did not apply a
  delete from its own user to another member's ordinary message.
- The Kupferdraht devices behaved as ADR-0028 then specified: user 1 (the
  author) rejected the delete; user 2's Kupferdraht device applied it (the
  broad clause (b), "self user — any target").
- Consequence: clause (b) is narrowed to what the protocol needs — a
  self-sent delete applies to another member's message only when that
  message is **ephemeral** (the own-device sync of a receiver-side
  expiry, §74: the official receiver's delete is addressed to self and
  sender). Only the webapp was tested; the ephemeral case of (b) rests
  on §74, not on a live run of this test.

## 92) A first MLS message right after an external commit can get a `500` (2026-09-30)

Browser smoke of Step 110 against schwarz-dev-wire: a fresh browser device
of user 2 (first setup) synced, joined the 1:1 by external commit
(`POST /v14/mls/commit-bundles`) and sent a text within the same second —
`POST /v14/mls/messages` answered `500 {"label":"server-error"}`. The same
device's next login (the device reused) sent to the same room without
error, and four other runs of the same sequence (two fresh devices, two
reused, one replaced) never saw it; the native CLI smokes have not either.
Kupferdraht surfaces it as `BackendRejected` and changes nothing — no local
state depends on the failed send. Watch for it; if it recurs, it is a
backend race between applying an external commit and accepting a message
of the new epoch, and a single retry of the send would cover it.
