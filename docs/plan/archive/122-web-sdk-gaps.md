# Step 122 — Small gaps of the browser surface

**Status: MERGED 2026-10-05** — commissioned 2026-10-02 (Markus: points
1 and 2 of the natrium-web SDK's gap list, measured on `19fbfea`; started
2026-10-05 as "121", renumbered — #111 holds that number). `b3f0ff4`.
Merged to `main` as PR #112 (`f1a95d2`); CI green. Smoke 2026-10-05
against the test stage (release wasm; CLI). Scope shipped: an own send
wakes the browser's sync loop; `pushState()`, `email`,
`storageEngine().location`, `retryAfter` on `RateLimited`;
`join_via_link` finds the link's room and drops `domain` (BREAKING).
Retrospective: `docs/LEARNINGS.md` §5.64. Archived with this header.

## Problem

The natrium-web SDK lists what it still misses on the browser surface.
This step takes the own-message delay and the small items; error variants,
the half-made first setup, the purge's lease, and re-measurements follow
separately.

1. **An own message shows late.** Since Step 118 a pass follows a frame,
   not a timer, and the backend sends the sender no frame for its own
   message. Measured here (2026-10-02, smoke page): the message is in
   `messages()` at once, `deliveryStatus: "submitted"` — but nothing tells
   the embedder to read again: no `onSync` until the next pass, up to a
   minute later (the socket renewal). The SDK works around it with a
   `syncOnce` after every send.
2. **`RateLimited` loses its `retryAfter`** at the browser boundary: the
   error detail carries it for `TooManyAttempts` only.
3. **No sign whether push runs.** A socket that opened says nothing, and a
   session without a device id or token never tries one, silently.
4. **No `email` on the browser `Client`** (the `PendingLogin` has it; a
   resumed session does not).
5. **`joinViaLink` needs the conversation's domain**, which the link does
   not carry (links are served from the account host). With the link's
   host the redeem succeeds and the lookup fails with `InvalidInput`. Worse,
   the lookup takes the **first** synced room of that domain — with any
   other room of the domain cached, it returns (and rejoins) that room and
   leaves the link's room without an MLS join.
6. **`storageEngine()` cannot say where the databases are** (local or on
   the storage server).
7. `syncOnce`'s doc comment (and two more) still say "OPFS-backed store".

## Design

- **D1 — a successful send rings the browser's sync loop.** Every message
  upload (`POST /mls/messages` — text, reply, edit, delete, reaction, file,
  knock, location, task, read receipt) signals the client once it
  succeeded. The browser loop waits for that signal as for a frame: it
  drains, and `onSync` follows — the embedder's cue to read again, in every
  tab. Signals coalesce; one that arrives while a pass runs is absorbed by
  that pass (its `onSync` comes after it), so the delivery receipts a pass
  sends itself never cause another pass. Native is unchanged (Markus,
  2026-10-05): its `Timeline` already reports the own message (local echo
  diff), and it has no tabs.
- **D2 — `RateLimited` carries `retryAfter`** (seconds, when known) like
  `TooManyAttempts`.
- **D3 — `client.pushState()`**: `"connected"` while the sync loop has an
  open socket, `"polling"` while it runs without one, `"stopped"` without a
  running loop. A method, not a getter: tab proxies snapshot getters when
  they get the handle, and this state changes.
- **D4 — `client.email`**: the account's email address, `undefined` when
  unknown — a getter (it does not change during a session).
- **D5 — `joinViaLink` finds the room by the link.** Before redeeming,
  `GET /conversations/join?key&code` names the conversation (its id); after
  the redeem and the room-list refresh the room is the one with that id. A
  room whose group is joined already is not joined again (a `204` redeem —
  already a member). No room with that id after the refresh: an error that
  says the join succeeded (`Generic`, not `InvalidInput`). A code the
  backend does not know fails before the redeem (`BackendRejected`).
  **The `domain` parameter goes** (Markus, 2026-10-05) — core, FFI, CLI
  (`--domain`), browser, tab mirror; BREAKING: `joinViaLink(code, key,
  password?)`. The link does not carry the domain (measured: the link is
  `https://account.<operator>/conversation-join/?key=…&code=…`), and a redeem
  only ever reaches the own backend's conversations (`open.md` §74).
- **D6 — `storageEngine()` adds `location`**: `"server"` for an account on
  the storage server, `"local"` otherwise. Browser only (native databases
  are local files).
- **D7 — doc comments** say "the account's store" instead of "OPFS-backed".

## Tests (red first)

- **Native, `tests/step_122_join_via_link.rs`:** the link's room is joined,
  not an unrelated room of the same domain listed first; a room joined
  already (`204`) is not joined again (no group info, no commit); a room
  missing after the redeem is `Generic`, not `InvalidInput`; a link the
  backend does not know (`404 no-conversation-code`) fails before the
  redeem. `tests/step_8_join_links.rs`: the redeem test takes the new
  signature and mounts the lookup.
- **Native unit, `client/sent_signal.rs`:** a successful upload notes one
  sent message (two uploads coalesce into one); a failed one notes nothing.
- **Browser, `tests/step_122_browser_surface.rs`:** `RateLimited` carries
  `retryAfter` (and none when unknown); push state `"stopped"` without a
  loop, `"connected"` / `"polling"` per the loop's socket, per account.
- **Tab mirror, `tests/step_94_tab_proxy_parity.rs`:** red until
  `pushState` and `email` are mirrored.
- Not testable without a backend and storage server: the loop waking on a
  send and recording its push state, `email` after a resume,
  `storageEngine().location` — **smoke** (test stage): send in one browser
  → `onSync` within a second; `pushState()` `"connected"`, `"stopped"` after
  `stop()`; `email` set; `storageEngine().location` `"server"`;
  `joinViaLink` from a second account with another room cached;
  `kupferdraht-cli join-via-link --code --key`.

## As built (2026-10-05)

- **D1:** `client/sent_signal.rs` — `SentSignal` (a `tokio::sync::Notify`:
  notes coalesce into one permit), noted by `Client::http_send` after a
  successful `POST /mls/messages`. The browser loop
  (`wasm/live_sync.rs`) waits for the bell or the signal
  (`wait_for_bell_or_send`; a send counts as `Wake::Rang`) and consumes a
  pending signal after each drained pass (`take`), so the receipts a pass
  sends itself cause no second pass. Native reads nothing of it.
- **D2:** `wasm::manager::set_error_detail` — `RateLimited` shares the
  `retryAfter` arm of `TooManyAttempts`. `wasm::js_error` is the mapping's
  public name (the browser test's seam).
- **D3:** `wasm/push_state.rs` — a per-account registry the loop writes
  after each reconnect check (`record_push_state`, skipped once the
  consumer stopped) and clears when it ends; `SyncHandle::stop` clears it
  at once. `Client.pushState()` reads it.
- **D4:** `Client.email` — `Account::email` (persisted with the self user
  at login, so a resume has it).
- **D5:** `wire/http/conversation_join.rs` — the lookup
  (`get_conversation_by_code`, `ConversationCoverView { id }`; the query is
  never logged and transport errors drop the URL) and the redeem, moved out
  of `conversations.rs`. `Client::join_via_link(code, key, password)` →
  `redeem_link` → `sync_metadata` → the room whose id's uuid is the
  conversation → `join_room` unless `room_group_joined`. Callers: FFI,
  browser, CLI (`--domain` gone), kupferdraht-tabs.
- **D6:** `storage::account_on_storage_server` → `storageEngine().location`.
- **D7:** doc comments in `wasm/` name the storage server and the local
  mode; the stale `https://wire.com/join/<key>/<code>` URL shape in
  `JoinLink`, the CLI help and the docs is replaced by the measured one.
- kupferdraht-tabs: `pushState()` (a call, never a snapshot), getter `email`,
  `joinViaLink(code, key, password?)`, `StorageEngineInfo.location`, and
  `InvalidRecoveryData`'s `reason` now lists Step 120's `oldDevice`.

## Smoke (2026-10-05, test stage — storage server 0.4.0, release wasm)

CLI (account 2, a member of the link's room; debug build):

| call | result |
|---|---|
| `join-via-link --code … --key …` (the link of room `0cdba7a0…`; the account has other rooms on the domain) | `GET` + `POST /conversations/join`, no group info, no commit — `room_id: 0cdba7a0…` (the link's room, not joined again) |
| `join-via-link` with an unknown code | `get_conversation_by_code 404 … no-conversation-code`, no redeem |

Browser (account 2, server mode, a fresh profile: login → first setup,
then the `echo` scenario of the smoke page):

| check | result |
|---|---|
| `pushState()` before `startSync` / 5 s after / after `stop()` | `stopped` / `connected` / `stopped` |
| `email` | the account's address |
| `storageEngine().location` | `server` |
| `onSync` after `sendText` resolved (socket open, so no 10 s poll) | **520 ms** (before: the next renewal, up to 60 s) |
| the message in `messages()` | at once, `submitted` |

Not smoked: a redeem by an account that is not a member yet (the
external commit after the redeem) — no second account with a free device
slot on the test stage; the integration test covers it, and the external
commit itself is Step 3's join.
