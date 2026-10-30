# Step 62 — Native `/v14/events` WebSocket push transport

> **Status:** merged to `main` 2026-08-14 (`3ea7b46` 62a → `a484e89`
> 62b + ADR-0017 → `67c2b03` 62c → `f17abbd` 62↔63 integration) —
> **archived 2026-08-18 as code-complete, upstream-disabled.**
> Live smoke 2026-08-12 verified the protocol half (upgrade, binary
> frames, tagged sync markers, ack path) against the own instance;
> event delivery never arrived because wire-server disables
> ConsumableNotifications **by hard-coded chart config** since chart
> 5.29/5.30 (PR #5116, WPB-23942 "prevent rabbit mq queues creation
> for client notifications", 2026-03-23) — PR #5116 hard-codes the
> flag, no public timeline for re-enabling it. The transport stays in the
> tree as an opt-in guarded by 62c's `server_supports_push_events()`
> (reads exactly that feature flag; the CLI warns); polling remains
> the default. Successor: the legacy cannon `/await` socket, which
> the same wire-server change routes *all* pushes through — spike
> green 2026-08-18 (observations §68), proposed as Step 73.
>
> Original scope decision by Volker 2026-08-11: **clean rebuild** (not a
> revival of the Step-5d code), **native targets first** (CLI / Kotlin /
> Swift; the WASM `live_sync` stays on polling, browser WebSocket is a
> follow-up step), smoke against **schwarz-dev-wire** (own v14 instance —
> the Wire-production RabbitMQ silence that killed Step 5d, CHANGELOG
> §30, does not bind there). Drivers: the kupferdraht-tui event loop wants
> push instead of the 2.5 s polling cadence, plus general
> latency/efficiency of a long-lived socket over tight polling.
>
> This step **reverses non-goal NG-14** (`open.md`) and the CHANGELOG
> §30 resolution ("drop the WebSocket transport entirely"). §30 itself
> names the re-entry condition this step now meets: *"Should Wire's
> RabbitMQ-push layer become reliable on a future account / team
> configuration, the WS path can be revisited; the client-side
> implementation was protocol-correct."* The reversal is an
> architectural decision → ADR + same-named `rules/` file land with 62b
> (via `new-adr`, discussed before writing).
>
> **Sub-steps:** 62a (wire-layer protocol: upgrade, frame parse, ack
> encode, socket primitive), 62b (SyncService push loop: dispatch,
> ack-after-persist, reconnect, CLI + FFI + docs), 62c (push-capability
> getter — added 2026-08-12 after the smoke surfaced the server-side
> `consumableNotifications` feature flag).
>
> **Historical names:**
> - "Step 62" — proposed 2026-08-11.
>
> **Branch / commit prefix:** `step/62a-*` / `step/62b-*`,
> `feat(step-62a): …`.

---

## Goal

A long-lived WebSocket against `GET /v14/events`
(ConsumableNotifications) as the push-mode receive transport on native
targets. The polling drain (`GET /v14/notifications`, Step 5/14b) stays
as the fallback and backfill path — backend-spec §11.5 makes REST
backfill an explicit alternative drain, and a consumer that never opens
the socket keeps working exactly as today.

## History (read before touching anything)

Two prior attempts, both dropped pre-merge:

- **Step 5a–c** (2026-04-26, CHANGELOG §23): first prototype, removed
  to keep the Step-5 debugging surface tight.
- **Step 5d** (2026-04-30, CHANGELOG §30): full rebuild, protocol
  verified live against Wire production — upgrade `101`,
  `synchronization` frame echo, accepted acks, 20 s server pings — but
  **zero `event` frames ever pushed** on that account/team
  configuration. Root cause located on Wire's RabbitMQ-routing side,
  not in Kupferdraht. Everything WS-specific was deleted; three things were
  deliberately kept and are still in place:
  - `sync_state.sync_marker_uuid` column (V7 migration) — reusable
    as-is;
  - `consumable-notifications` capability in the `POST /v14/clients`
    registration body — every registered Kupferdraht device already
    declares it;
  - `Capabilities` field on `RegisterClientResponse`.

This step is a **clean rebuild against `docs/backend-spec.md`
§11.2–§11.5**, which was frozen 2026-07 precisely for this purpose
("live-derived from Wire production and could not be reconstructed
later"). Kalium is not consulted (independence rule; the spec already
encodes the observed protocol).

## Contract (normative references)

- **Upgrade:** `GET /v{N}/events?client=<id>&sync_marker=<uuid>&access_token=<token>`
  → `101`. Auth via `access_token` **query parameter** (Wire strips
  `Authorization` headers on upgrade — backend-spec §11.2, LEARNINGS
  §1.6; the Bearer header MAY additionally be sent, the query param is
  the one that must be there). The `wss://` base is the per-session
  `websocket_url` persisted since V1
  (`storage/records.rs::SessionRecord.websocket_url`).
- **Frames** (JSON text, one `ConsumableNotification` per frame,
  backend-spec §11.2):
  - `{"type":"event","data":{"delivery_tag":<int>,"event":{"id":"…","payload":[…]}}}`
  - `{"type":"synchronization","data":{"marker_id":"<echo of sync_marker>"}}`
    — sent once, after the pre-connection queue has drained ("caught
    up" signal).
  - Unknown `type` values: parse the envelope, log, discard — never
    tear down the socket (CLAUDE.md unknown-types rule applied to
    frames).
- **Acks** (§11.3): client → `{"type":"ack","data":{"delivery_tag":<int>,"multiple":<bool>}}`;
  `delivery_tag` is connection-scoped, strictly increasing from 1;
  unacked deliveries are redelivered on the next connection (same
  notification `id`, new `delivery_tag`).
- **Reconnect** (§11.5): fresh `sync_marker` UUID per connection;
  backend replays unacked FIFO, then the marker frame, then live.
- **Heartbeat:** server pings ≤ 30 s (Wire prod: every 20 s); client
  must pong (tungstenite answers pings on read — pinned by a test, not
  assumed).
- **One socket per device:** a second connection for the same client
  terminates the first (§11.2). The client therefore treats an
  unexpected close as "reconnect with backoff", not as an error loop.

## Sub-step 62a — wire-layer protocol (`wire/websocket.rs`)

Transport only, Wire field names verbatim, no business logic, no
storage, no MLS (ADR-0009 layer rule). `cfg(not(target_arch =
"wasm32"))` throughout — the wasm build must not see
`tokio-tungstenite`.

New crate-internal surface (no `pub` API growth in 62a):

- `ConsumableNotification` frame model: `Event { delivery_tag, event:
  QueuedNotification }` (reuses the existing REST DTO for the inner
  notification — same shape), `Synchronization { marker_id }`,
  `Unknown` (tolerated).
- `parse_consumable_frame(&str) -> Result<ConsumableNotification, KupferdrahtError>`
  — `Encoding` on malformed JSON / missing fields, `Unknown` (Ok) on
  unrecognized `type`.
- `ack_frame(delivery_tag, multiple) -> String` — §11.3 shape.
- `events_url(websocket_base, api_version, client_id, sync_marker,
  access_token) -> Result<Url, KupferdrahtError>` — `ws`/`wss` scheme
  required (`InvalidInput` otherwise), version path + the three query
  params.
- `EventSocket` — `connect(url)`, `next_frame()` (Ok(None) on clean
  close), `send_ack(delivery_tag, multiple)`. Thin: connect + read +
  write + pong; no reconnect logic here (that is 62b's loop).

Error mapping reuses existing variants: `Network` (transport),
`Encoding` (frame shape), `InvalidInput` (caller errors). No new error
enum.

**TDD list (red phase = these tests + `todo!()` stubs):**

Known-answer tests (frames verbatim from backend-spec §11.2/§11.3):

1. `event_frame_parses_delivery_tag_and_notification`
2. `synchronization_frame_parses_marker_id`
3. `unknown_frame_type_is_tolerated_not_error`
4. `malformed_event_frame_is_encoding_error`
5. `ack_frame_matches_spec_shape` (compared as JSON values, not
   strings)
6. `events_url_appends_version_path_and_auth_query`
7. `events_url_rejects_non_ws_scheme`

In-process WebSocket server tests (`tokio_tungstenite::accept_hdr_async`
on a loopback listener — httpmock cannot speak WS):

8. `connect_sends_client_marker_and_token_query_params_on_upgrade`
9. `frames_arrive_in_order_event_then_synchronization`
10. `send_ack_reaches_server_verbatim`
11. `server_ping_is_ponged_automatically`

Dependency: `tokio-tungstenite` (rustls, webpki roots — no native-tls,
Stack rule) under the native-target dependency block; `cargo deny`
must stay green (MIT — verified at the gate).

## Sub-step 62b — push loop, CLI, FFI, docs

- `SyncService::run_push` (internal): connect via 62a, then per
  `event` frame run the **existing** dispatch pipeline
  (`dispatch_notification` — MLS decrypt, handlers, cursor persist)
  and **ack only after** the dispatch + cursor persist completed.
  **Load-bearing invariant, test-pinned:** an event acked before
  durable local processing is gone forever on crash; a lost MLS
  `Commit`/`Welcome` bricks the device's ratchet (backend-spec §11.1).
  Polling tolerates this via the non-consuming cursor; the socket ack
  is consuming, so the order is the one new correctness obligation of
  this step.
- Redelivery dedup by notification `id` against the persisted
  `last_event_id` path (redeliveries carry a *new* `delivery_tag`, so
  the tag is not the dedup key).
- `sync_marker` handling per §11.5: fresh UUID per connection,
  persisted in `sync_state.sync_marker_uuid`; the `synchronization`
  echo flips the service from "replay" to "live" (observable in the
  summary/logs).
- Reconnect with capped exponential backoff; an orderly close or
  transport error never surfaces as a hard error to the consumer —
  the loop reconnects; per-event failures keep folding into
  `on_sync_error` exactly as in the polling drain.
- Public API (matrix-rust-sdk naming, minimal): extend the existing
  `sync*` family rather than a parallel one — `SyncSettings` gains the
  transport choice (polling stays the default; exact shape decided in
  62b's red phase with the failing tests on the table).
- CLI first consumer: `kupferdraht-cli sync-stream --transport
  websocket|polling` (default `polling` until the smoke passes).
- FFI parity in the same commit for every changed `pub` symbol +
  `api-mapping-matrix-rust-sdk.md` entry.
- Docs in the same commit: `open.md` NG-14 falls, README scope
  ("polling only" → push), backend-spec §11.2 reference-client status
  note, `docs/library.md` typing caveat (the Step-21 receive path goes
  live), `docs/cli.md`, CHANGELOG. ADR + `rules/` file for the §30
  reversal.

## Sub-step 62c — push-capability getter

Live finding (observations §66): whether `/v14/events` delivers
anything is a **server-side switch** — the `consumableNotifications`
team feature (backed by wire-server's root Helm flag, default
`false`). A consumer choosing between `SyncTransport::WebSocket` and
`Polling` needs to read that switch; Kupferdraht's Step-10 server-config
awareness already caches the feature-configs table at login and keeps
it fresh via `feature-config.update` events, so this is one typed
getter in the Step-10e pattern:

- `Client::server_supports_push_events() -> Result<bool, KupferdrahtError>`
  — `true` iff the cached `consumableNotifications` entry has
  `status: "enabled"`. **Absent entry → `false`**: deliberately the
  opposite default direction to `file_sharing()`'s permissive
  `EnabledAll` — recommending WebSocket against a server that never
  delivers silently breaks receive, while falling back to polling
  always works.
- FFI mirror (delegate in `ffi/admin.rs`, same commit).
- CLI as first consumer: `sync-stream --transport websocket` warns at
  startup when the getter reports `false` (the stream would connect
  but stay silent — exactly the smoke's failure mode).
- Docs: `library.md` transport-choice recipe
  (`if client.server_supports_push_events() { WebSocket } else
  { Polling }` — deliberately no automagic `Auto` variant),
  `api-mapping` row, CHANGELOG append to the Step-62 entry.

**TDD list:** enabled → `true`; disabled + locked (the live 2026-08-12
fixture verbatim) → `false`; entry absent → `false` (conservative
default pinned).

## Out of scope

- Browser/WASM WebSocket (`web_sys::WebSocket`, no-custom-header
  handshake) — follow-up step; `wasm/live_sync.rs` keeps polling.
- Debugging Wire production's RabbitMQ routing — smoke is against
  schwarz-dev-wire; a prod datapoint is optional curiosity, not an
  acceptance gate.
- Push notifications (APNs/FCM) — unrelated; the team's backend
  redirect covers that.
- Removing the polling path — it stays as fallback + backfill (§11.4).

## Verification / smoke (phase 9)

> **Status 2026-08-12:** partially passed, then **blocked server-side**
> (`docs/observations.md` §66). Live against the self-hosted instance:
> upgrade 101 ✓, marker enqueue/echo/ack ✓, ack + redelivery semantics
> ✓ (two protocol deviations from the frozen spec were found live and
> fixed client-side: binary frames, acked `delivery_tag`-carrying
> markers — backend-spec §11.2 updated). But the deployment's gundeck
> never publishes any event (durable or transient) into the consumable
> queue, so acceptance items 2–4 cannot pass anywhere yet — same
> symptom as CHANGELOG §30 on production. Blocked on backend ops
> (wire-server version/config with gundeck→RabbitMQ publishing,
> upstream #4626); re-run `scripts/smoke-step62.sh` afterwards.

Against **schwarz-dev-wire**: `kupferdraht-cli --api-url … sync-stream
--transport websocket` on device A; send from device B. Acceptance:

1. `synchronization` frame observed after backlog replay;
2. a live text message arrives end-to-end (decrypt + handler + ack;
   verify the queue actually shrinks server-side, i.e. acks consume);
3. a `conversation.typing` event arrives live (the transient-event
   proof — impossible on polling by construction);
4. kill the connection mid-stream, reconnect: unacked events are
   redelivered once and deduped, cursor intact.
