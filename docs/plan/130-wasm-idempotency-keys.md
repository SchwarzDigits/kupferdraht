# Step 130 — Idempotency keys for wasm mutations (a retry across a leader change)

**Status: in progress** — **130a merged 2026-11-04** as PR #29
(`224cf7a`, feature commit `2bb4b7a`): gate green (wasm32 lines
included); CI green, the tabs failover against the stub running for the
first time in `step_94_leader_tab` (18 tests, headless Firefox); smoked
2026-11-04 against the self-hosted stack with the CLI (a key sent twice
reached the second device once under one id; a keyed reaction repeated
stayed on, a new key toggled it off; a message key reused for a reaction
was refused); the browser end-to-end path (takeover → the new leader
re-drives the orphaned row) is left to the embedder's app, asked for in
the PR. Next: 130b, after Step 141 and before the v0.2.1 release; 130c
changes values the browser reads and waits for 0.3.0. Commissioned
2026-11-01 (transferred from open.md's former "Proposed steps", proposed
2026-09-25 in the Step 94 retrospective; ADR-0026 option L3-C, deferred
there to "a later step").

**Taken 2026-11-04 (Volker, with both browser embedders):** route (c),
keys first. The embedders agreed on a trailing options object, on the
automatic re-issue of mutations that are idempotent at the backend, and on
`createRoom`, the purging logout, `deleteDevice` and the setup / key-file
operations rejecting as before; natrium-web's order: keys first (incl.
`sendFile`, one key per call), the wasm resend path later;
`toggleReaction` keyed and never re-issued without its key (a repeated
toggle flips it back); the key on the handle whoever generated it; the
three-class table published and pinned by the parity test. Volker: the key
enters the core API (native tests can count the uploads — the browser
suite runs in CI only and has no backend there); keys live 28 days (the
`zuid` lifetime, backend-spec §5.1), swept when a key is written;
`invite`, `kick`, `leave`, `deleteRoom`, `revokeJoinLink` and `setHandle`
stay `mutation`; three sub-steps:

- **130a — idempotency keys** (merged, PR #29): the core API, V32, the wasm
  options, the three-class table.
- **130b — the Step-79 resend path on the wasm surface:** `failedSends`,
  `resend`, `resendFailed` and their tab mirrors (`idempotent`: the same id
  re-driven).
- **130c — open.md §85:** `undefined` → `null` on the item views
  `failedSends` returns too; a `Changed` entry and a `.d.ts` review.

## Problem

ADR-0026 (Step 94) chose L3-A for the thin-client tabs: a read in flight
when the leader tab changes is re-issued to the new leader; a mutation in
flight (`sendText`, `sendReply`, `sendFile`, `toggleReaction`, `editMessage`,
`redact`, `markAsRead`, `createRoom`, `createDm`, `invite`, `kick`, `leave`,
…) rejects with a typed `LeaderChanged` and is never retried by
`kupferdraht-tabs`, because the module cannot know whether the old leader's
request reached the backend. Honest, and the right default without more
state — but every embedder has to write the retry, and a blind retry can
send a message twice.

Native consumers have Step 79's answer: a failed send is a `Failed` row,
`Room::failed_sends` lists them, `Timeline::resend` / `resend_failed`
re-drive them under the same message id. The wasm surface has none of it
(`wasm/manager.rs` exposes neither `failed_sends` nor `resend`), so the
browser embedder has neither a safe retry nor a visible failed row to offer
the user.

## Scope

Phase 1 picks the route, with the embedder of the browser surface in the
loop:

- **(a) L3-C, idempotency keys:** a client-chosen key per mutating call (an
  optional last argument or an options object on the wasm surface), stored
  with the call's outcome in the leader's account store (a new table, keyed
  per account; retention to decide — a sweep after a bounded age). The same
  key answers the stored handle without a second upload, a new key re-runs.
  `kupferdraht-tabs` then retries a keyed mutation on the new leader instead
  of rejecting: `protocol.js`'s classification gains a third class, the
  parity test of Step 94 pins it, `index.d.ts` carries the argument.
- **(b) The Step-79 resend path on the wasm surface:** `failedSends` and
  `resend` / `resendFailed` on the wasm `Room` / `Timeline`, their tab
  mirrors (`METHODS` rows, proxy methods, `index.d.ts`), the doc homes. The
  embedder shows the failed row and re-drives it; no automatic retry.
- **(c) both:** (b) first as the visible fallback, (a) for the silent case.

The recommendation going into phase 1 is (c): (b) is parity with native and
small, (a) is what makes a leader change invisible. Whether the key enters
the core API (native and FFI parity, so a Kotlin / Swift consumer retries
after a network error the same way) or stays a wasm-only argument is the
phase-1 question with the widest reach; ADR-0026 gets a revision entry
either way.

## Out of scope

- An offline queue (ADR-0026 L3-A: no call waits for a leader beyond the
  bounded first announcement).
- Login, device registration and logout: a `PendingLogin` cannot move
  between leaders (ADR-0026) and gets no key.

## TDD sketch

- core: the dedup table's migration; a mutation with a key stores its
  outcome; the same key returns the stored handle and the backend sees one
  upload (httpmock request count); different keys send twice; a key of
  another account is not visible — red.
- wasm surface: the parity pins of `step_94_tab_proxy_parity.rs` extended
  for the new argument or methods; the browser tests (`scripts/wasm-test.sh`,
  run in CI) cover a keyed send answered from the store — red.
- tabs: a keyed mutation in flight during a leader change resolves on the
  new leader with the stored handle, an unkeyed one still rejects
  `LeaderChanged` — a browser test in CI, or failing that a smoke item as in
  Step 125.

Smoke (two tabs on the test stage): a send in flight while the leader is
evicted (the Step-100 takeover) arrives exactly once.

## 130a — design

- **D1 — one keyed entry on `Timeline`:** `Timeline::send_keyed(
  idempotency_key, content: OutgoingContent)` → `MessageHandle`, with
  `OutgoingContent` = `Text { body, mentions }` (joins the room's MLS group
  first when it is missing, as `send_text_auto_join`), `Reply { parent_id,
  body }`, `Knock { hot_knock }`, `Location { latitude, longitude, name,
  zoom }`, `File { bytes, filename, mime_type }` (the in-memory path of
  `sendFile`), `Reaction { item_id, emoji }` (a toggle). matrix-rust-sdk:
  `Room::send(content).with_transaction_id(txn_id)`. FFI mirror on the FFI
  `Timeline`; `OutgoingContent` is a `uniffi::Enum`.
- **D2 — the key:** 1–128 characters, else `InvalidInput`; chosen by the
  caller. The wasm binding and the tab proxy generate a UUID when
  `options.idempotencyKey` is missing; the wasm handle of a keyed call is
  `{ messageId, sentAtMs, idempotencyKey }`.
- **D3 — V32 `send_keys`** in the account store: `idempotency_key` (PK),
  `room_id`, `kind` (`message` | `reaction`), `message_id`, `sent_at_ms`,
  `created_at_ms`. A message key is written in the same transaction as the
  `Sending` row (and its mentions or attachment), before the MLS upload; a
  reaction key in the same transaction as the reaction set, after the
  upload. Every keyed write first deletes the keys older than 28 days
  (`SEND_KEY_RETENTION_MS`). `Store` gains `send_key`,
  `insert_timeline_event_keyed`, `replace_user_reactions_keyed` with
  default bodies (custom-store pattern: a custom store keeps no keys and
  every call sends).
- **D4 — a known key** answers instead of sending: another room or another
  kind → `InvalidInput`; a reaction → the stored handle, never toggled
  again; a message whose row is `Failed`, or `Sending` and not in flight →
  the Step-79 resend under the same id; a message in flight on this client
  → `InvalidInput` (as a concurrent resend today); any other row state, a
  redacted row or no row → the stored handle. An unknown key sends; that is
  safe because the row and its key precede the upload. The Step-40
  new-device wipe clears `send_keys` with the rows (a wiped `Failed` row
  must not answer as sent).
- **D5 — the wasm surface:** `sendText`, `sendReply`, `sendKnock`,
  `sendLocation`, `sendFile`, `toggleReaction` take a trailing
  `options: { idempotencyKey? }` and call `send_keyed`.
- **D6 — the three classes** (`protocol.js`, the parity test, the tabs
  README, an ADR-0026 revision + `rules/0026`): `keyed` — the six above,
  re-issued to the new leader with their key, the proxy adding a key when
  the caller gave none; `idempotent` — the reads plus `markAsRead`,
  `markAsUnread`, `startTyping`, `stopTyping`, `redact`, `createDm`,
  `renameRoom`, `setRoomAccess`, `allowGuests`, `setDisplayName`,
  re-issued; `mutation` — everything else, `LeaderChanged` as before.
  `invite`, `kick`, `leave`, `deleteRoom`, `revokeJoinLink` stay
  `mutation`: a repeat after a call that landed rejects ("not in cache",
  404, an MLS add of a member already in the group), and an honest
  `LeaderChanged` beats a false error. `setHandle` stays `mutation` until
  Wire's answer to the own handle set again is observed.
- **D7 — CLI:** `send --idempotency-key <KEY>` and `react
  --idempotency-key <KEY>` call `send_keyed` (a message and a reaction).

## 130a — tests (red first)

- **Native, `tests/step_130a_idempotency_keys.rs`** (the Step-79
  fixture: a client welcomed into a peer's group, uploads captured off
  `POST /v14/mls/messages`): a keyed text uploads once and the same key
  answers the same handle with one row; two keys upload twice; a known key
  with a `Failed` row resends under the same id; a keyed reaction applied
  twice is one upload and stays on, a new key toggles it off; reply,
  knock, location and file each dedupe; a key of another kind and an empty
  or 129-character key are `InvalidInput` with no upload; the store forgets
  a key 28 days after the next keyed write and keeps it one millisecond
  short of that.
- **Pins, `tests/step_130a_keyed_surface_pins.rs`:** `protocol.js`'s
  classes are the three-class table (the six keyed methods, the ten
  re-classified `idempotent` ones, the six that stay `mutation`); the tabs
  README publishes the same table; the six wasm methods end in
  `options: JsValue` and hand back the key; the proxy adds a key to each
  of the six; the failover re-issues `keyed`; `index.d.ts` types the
  options and the keyed handle; the FFI `Timeline` mirrors `send_keyed`;
  the CLI has `--idempotency-key`; V32 is pinned in
  `migration_immutability.rs`; the doc homes (ADR-0026 revision,
  `rules/0026`, bindings, library, cli, ARCHITECTURE §7, api-mapping,
  CHANGELOG) name the keys.
- **Step 94 parity test:** a fourth kind, `keyed`.
- **Browser, Step 94 driver (CI):** `electionAndFailover`'s `sendText` in
  flight now resolves on the new leader with the caller's key; a
  `createRoom` in flight still rejects `LeaderChanged`; a keyed call
  without a key resolves with a generated one.

## 130a — smoke

Taken 2026-11-04 (Volker): the CLI smoke only; the browser end-to-end
path is confirmed through the embedder's app (asked for in the PR).
The plan was: two tabs on the local stack (`opfs-smoke/tabs.html`), a
send in flight while the leader is evicted (the Step-100 takeover)
arrives exactly once; the CLI `send --idempotency-key k` twice shows one
message on a second client.
