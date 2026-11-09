# Step 151 — The time of sending: `sent_at_ms` is Wire's, not this device's

**Status: in progress** on `step/151-send-time`; phase 1 taken and red tests approved by
Volker on 2026-11-09. Commissioned 2026-11-09 by Volker ("das ist wichtig"), ahead of
Step 146, from what Step 145 found.

## Problem

An inbound row's `sent_at_ms` is `now_ms()` at dispatch (`sync/dispatch/text.rs`,
`asset.rs`, `mod.rs`), and an own row's the device's clock at the send. The event's
`time` and the answer's `time` are on no row.

1. **A device back from offline shows the wrong time.** Everything a catch-up drain
   delivers shows the time of the sync: last night's messages say this morning.
2. **A reply can fail to verify.** `Text.quote.quoted_message_sha256` hashes the
   parent's body and its send time in seconds (`docs/observations.md` §33). Kupferdraht
   hashes `sent_at_ms`, so a reply to an inbound message carries the receive time. Wire's
   clients hash their own record of the parent, the backend's time: the two differ as
   soon as receipt falls into another second, and always after a catch-up. Kalium sets
   `isVerified` from that comparison.
3. **Step 145's items** have the same flaw, by that step's decision: one clock per
   timeline.

## Phase 1, measured 2026-11-09

**Wire's clients, protocol behaviour only (ADR-0009):**
- `ReceivedMessage.kt` (Kalium): `quotedReference.copy(isVerified =
  hash.contentEquals(originalHash?.sha256Digest))`, the digest over the stored message's
  `date` and content. So Wire's Android app notices a wrong hash.
- `QuoteDecoderMiddleware.ts` (webapp): keeps `quotedMessageSha256` and reports only
  `MESSAGE_NOT_FOUND`; this path shows no comparison. So the webapp does not notice, which
  is why it went unseen.
- `MessageRepository.ts` (webapp): after a send, `updateMessageAsSent` sets the message's
  `time` from the backend's `sentAt`. An own message takes the server's time as well.

**The Swagger:** `Event` requires `time`; `MLSMessageSendingStatus`, the answer to
`POST /mls/messages`, requires `events` and `time` (`UTCTimeMillis`). The wire-server-rs
stack builds both from one `now` (`crates/galley/src/mls.rs`).

**Measured against a real Kalium client, on `main` before this step.** Volker's point: the
other side can be a Natrium client, which wraps Kalium. Natrium (Kalium `digits/0.0.7`, API
v15) and Kupferdraht (API v14) in one MLS room on the team's dev backend (2026-11-09):

| Case | Result |
|---|---|
| Natrium sends, Kupferdraht is away for a minute, then replies | Natrium gets the reply and its `quoted` id, but `previewText` is `null`: Kalium's `QuotedMessageDetails.Invalid` |
| The same message on both sides | 67 s apart: Kupferdraht shows the sync |
| Kupferdraht replies to its own message | preview shown: same second, so it does not show the defect |
| Natrium replies to Kupferdraht | Kupferdraht shows the reply with its parent |
| Natrium creates a room and adds Kupferdraht (Step 145) | Kupferdraht shows `* … added …` |

So the defect is real and visible to a user of a Kalium client: the quoted text is gone. The
first two rows are this step's red, and the whole table is its smoke. Natrium's own code does
not change; the test harness lives outside its repository.

**The clock that must not change.** The order of a timeline and the read marker need
one clock, or Step 145's defects return: an item or message with a server clock ahead
sets the marker past the next message, and a catch-up mixes with what was sent here.

## Decided (Volker, 2026-11-09)

1. **`sent_at_ms` becomes the truth, a new `arrived_at_ms` holds the position.**
   - `sent_at_ms`: an inbound row's event `time`; an own row's answer `time` once the
     upload succeeded, the device's clock until then and on a failure.
   - `arrived_at_ms`: when the row entered this device's timeline. `ORDER BY`, the unread
     queries, `set_read_marker` and the task fold read it.

   The alternative, a display column beside `sent_at_ms`, keeps the diff smaller but
   leaves the public field meaning "received", and every consumer and the quote hash
   would have to learn the new one.
2. **Public:** `TimelineItem::arrived_at_ms` (FFI too, browser `arrivedAtMs`), because
   `Room::read_marker` and `lastReadAtMs` stay on that clock and an embedder comparing them
   with `sentAtMs` would mix clocks. `Message::sent_at_ms` beside `received_at_ms` (browser
   `onMessage`'s `sentAtMs`). `MessageHandle::sent_at_ms` is the answer's time too.
3. **BREAKING, in the coming 0.5.0:** `sent_at_ms` changes meaning, and
   `TimelineEventRecord`, `TimelineItem` and `Message` grow a field.
4. **A `time` that cannot be read** falls back to this device's clock: never a failed sync
   or send.
5. **Old rows stay as they are.** Migration V35 sets `arrived_at_ms = sent_at_ms`, so
   positions and markers hold. Their receive time cannot be corrected: the server's was
   never stored.
6. **Step 145's items** get the event's `time` the same way.

## Scope

1. **Parse:** `wire/time.rs`: Wire's `UTCTimeMillis` to milliseconds; the `time` crate,
   already in the graph, gains `parsing` in the core.
2. **Inbound:** the envelope reads `time`; every row of `dispatch/` (text, ephemeral, asset,
   knock, location, task, case, marker) and `dispatch/room_event.rs` stores it.
3. **Own:** `send_mls_message` returns the answer's `time`; `finish_send` stores it;
   `MessageHandle` reports it; a resend moves the position and takes the new time.
4. **Storage:** V35, `arrived_at_ms`, its index replacing `sent_at_ms`'s, and the queries
   above.
5. **Surface:** FFI, wasm, `kupferdraht-tabs/index.d.ts`, CLI.
6. **Docs:** `docs/library.md`, `docs/bindings.md`, `docs/ARCHITECTURE.md` §7,
   `docs/wire/api-notes.md`, `docs/observations.md` §33's correction,
   `docs/wire/kalium-readings.md`, `docs/api-mapping-matrix-rust-sdk.md`,
   `docs/THREAT_MODEL.md` (the time shown is the backend's statement and can be
   backdated; order and unread cannot), `CHANGELOG.md`; Step 145's sentences on the clock.

## Out of scope

- `edited_at_ms`, `redacted_at_ms` and the receipt times stay this device's clock. They are
  shown as "edited", not as a moment, and no hash reads them.
- `received_at_ms` keeps its meaning.
- Checking an inbound quote's hash, as Kalium does.

## Residual risks

- A backend can backdate what a timeline shows. It cannot move a message or make it read:
  order and unread are on the device's clock. MLS does not authenticate the time.
- Between the persist and the answer an own row shows the device's clock.
- A message older than its neighbours by `sent_at_ms` can stand after them: it arrived
  later. So a timeline's times need not rise, and a UI must not sort by them.

## TDD sketch

Red in `tests/step_151_send_time.rs`, `wire/time.rs`.
- Inbound: each row kind stores the event's time; `on_message` carries both times; an
  unreadable time falls back.
- Position: arrival order against the times; the unread count and the marker with a
  server time in the past and in the future; old rows keep their place after V35.
- Quote: a reply to an inbound and to an own message hashes the backend's time, known
  answers computed outside Kupferdraht.
- Own: each send kind takes the answer's time, with the handle; no readable answer and a
  failed send stay on this clock; a resend moves.
- Room events: `sent_at_ms` is the event's, the position stays where it arrived.
- Surface: FFI fields, browser pins.
- Smoke: `kupferdraht-smoke.sh --throwaway` 24/24, then the Natrium table above against the
  dev backend, the first two rows green. It is a condition of the merge (Volker).
