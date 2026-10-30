# Step 89 — Ephemeral messages (send + receive toleration)

> **Status:** merged to `main` as `7ed0d3c` (2026-09-17, fast-forward
> of `157a643` feature + `7ed0d3c` V1b; live-smoked 2026-09-15/16 on
> schwarz-dev-wire — scripted kupferdraht→kupferdraht round-trip incl. the V1b
> delete reaching the sender, kupferdraht→webapp delivery; the webapp
> eyes-on check — countdown renders, vanishes without a second send,
> webapp→kupferdraht with a typed timer — is Volker's, pending). Scope:
> `Timeline::send_ephemeral_text(body, ttl)` over Wire's `Ephemeral`
> wrapper (field 18, `Text` inner arm; 1 s … 365 days), receive
> toleration as a `Text` item / `Message` decorated with
> `expires_at_ms` (V27 column; other inner arms tolerated-unknown),
> local self-redaction on expiry via per-row timers + end-of-cycle
> sweep + resume sweep, and — as receiver — the `MessageDelete`
> broadcast official senders wait for (V1b, decided 2026-09-16). Own
> expired rows stay local; `DeleteForMe` into the self-conversation
> waits for Step 86. CLI `send --ephemeral <secs>`; FFI + wasm views
> mirrored. 12 integration tests + codec/sweep pins. Closes the
> open.md §64 "Ephemeral receive-side toleration" item; PoC stage 1
> of the one-time-code use case.

Orientation decisions (2026-09-15, worktree instance, Phase-4 hard
stop waived by Volker), binding for the implementation: (1) proto
shape verified
(observations §74) — field 18 wrapper, `required int64
expire_after_millis`, inner `Text = 2`, arms 3–6 tolerated-unknown;
(2) **API form = separate `Timeline::send_ephemeral_text(body, ttl:
Duration)`** — matrix-rust-sdk has no self-destructing content to
align with, `send_text` stays untouched, and the wrapper is a distinct
`GenericMessage` variant with its own contract (an options struct
would have made every plain send carry an ephemeral knob); (3) **timer
start V1 = dispatch/send** — the official receiver starts on viewport
entry, which a headless client cannot observe; starting earlier is the
safe direction (delta documented in `message-types.md`, `library.md`,
observations §74); (4) **expiry V1a = local self-redaction** via the
`MessageDelete` tombstone, **plus V1b (decided 2026-09-16, Volker:
"if we build it, build it completely"):** Kupferdraht-as-receiver
broadcasts the expiry as a `MessageDelete` into the room — the
official-client split (receiver enforces network-wide, sender only
marks its copy and waits for that delete; wire-webapp observed,
Kalium confirmed, observations §74). Own rows stay local-only.
Best-effort upload (Step-25b receipt precedent). Not mirrored: the
`DeleteForMe` into the self-conversation for the receiver's other
devices — needs Step 86. Storage: V27
`timeline_events.expires_at_ms`.

Commissioned 2026-09-15 (Volker). Driver: the one-time-code
use case — a native ephemeral
send collapses the OTP service to a stateless one-shot; the
timed-MessageDelete workaround would re-introduce service state
and a second delivery. Receive-side toleration is the open.md §64
candidate ("display + honor the local delete timer", the ADR-0011
receive-toleration precedent). **Functional-scope extension**:
README + CLAUDE.md Functional Scope + `docs/wire/message-types.md`
move in the same commit (charter rule); Ephemeral leaves the
"reserved, never emitted" list.

## Wire format (orientation to verify, not assume)

`Ephemeral` is content-oneof field **18** of `GenericMessage`
(today decoded-to-None per our reserved list). Expected shape per
Wire's public proto: a wrapper `Ephemeral { expire_after_millis,
oneof content { Text … } }` — verify the exact field numbers and
which inner arms exist against Wire's generic-message proto
(protocol observation only; Kalium may be consulted per the
reference rules, never copied). Also verify Wire's TIMER SEMANTICS
on official clients (timer from delivery vs. from read) — the
receive toleration should match what official senders expect.

## Scope

1. **Proto**: add the `Ephemeral` variant to the content oneof
   with the `Text` inner arm (V1); other inner arms decode
   tolerantly to the unknown-variant path (envelope rule).
2. **Send**: `Timeline::send_text` gains the ephemeral option —
   API shape decided in orientation against matrix-rust-sdk
   naming (separate `send_ephemeral_text(body, ttl)` vs. an
   options struct). CLI: `send --ephemeral <secs>`. FFI mirror in
   the same commit. Codec pin in `wire/proto.rs` unit tests
   (field 18 set, TTL carried, inner Text intact — the
   only enforceable place, MLS ciphertext is opaque outside).
3. **Receive toleration**: decode inbound Ephemeral(Text) into a
   timeline item carrying body + TTL metadata; persist; run the
   LOCAL delete timer (reuse the MessageDelete/redaction
   mechanics for the expiry — the tombstone path exists). Timer
   start per the verified Wire semantics; if that needs read
   receipts, V1 falls back to delivery-start and documents the
   delta honestly. Non-Text inner arms stay tolerated-unknown.
4. **Full round-trip + live smoke**: kupferdraht → official Wire
   client (webapp on schwarz-dev-wire): countdown renders, message
   disappears WITHOUT a second send; official client → kupferdraht:
   item appears and self-redacts after TTL. This smoke doubles as
   PoC stage 1 of the OTP design brief.

## Out of scope

- Ephemeral wrapping of non-Text content (assets, knocks) — V1 is
  the OTP-shaped Text case; arms extend later on demand.
- Any OTP logic (generation, validation, rate limiting) — lives
  in the consumer service.

## TDD sketch (red first)

- Codec: build ephemeral text → field 18 present with TTL, inner
  Text intact, top-level oneof otherwise unset; decode round-trip.
- Dispatch: inbound Ephemeral(Text) → timeline row with TTL
  metadata, `on_message` fires; inbound Ephemeral(unknown-arm) →
  tolerated, no row loss elsewhere in the frame.
- Expiry: row self-redacts after TTL (tombstone semantics,
  redaction-refold preservation from the §72b family).
- Preservation: plain text send/receive unchanged; the reserved
  list in `message-types.md` shrinks by exactly one entry.

## Implementation notes + retrospective (2026-09-15, worktree instance)

**Landed shape.** `timeline/ephemeral.rs` (send + sweep + timers),
`sync/dispatch/ephemeral.rs` (inbound unwrap, reuses the text tail
`dispatch_inbound_text_message`), V27 `timeline_events.expires_at_ms`
with a partial index, `Store::expire_ephemeral_messages` /
`pending_ephemeral_expiries` (SQLite; IndexedDB keeps the default
no-op — wasm is sweep-only and its store implementation is left to
the WASM rework), `TimelineItem::expires_at_ms` /
`Message::expires_at_ms` (FFI + wasm views mirrored), CLI
`send --ephemeral <secs>` + `timeline` suffixes. Nine integration
tests (`step_89_ephemeral_messages.rs`), codec pins, sweep unit
tests. `SyncService::end_dispatch_cycle` now fuses the Step-74
removal flush with the expiry sweep at all three cycle ends.

**Simplifications / debt, honestly.**
- *Failed sends carry no timer* (deliberate — mirrors the official
  sender starting on the backend ack and keeps the body for Step-79
  recovery); a later resend must re-stamp.
- *Timer start = dispatch* on receive: an official receiver starts on
  viewport entry. A V2 could move the start to `mark_as_read` and use
  the `expires_at_ms IS NULL until started` shape the column already
  admits — no schema change needed.
- *V1b built (2026-09-16):* the sweep broadcasts a `MessageDelete`
  for expired inbound rows via `Client::upload_message_delete` (the
  network half split out of `redact_message`); one MLS message per
  expired inbound ephemeral per receiving device. No retry on a
  failed upload (logged; the local tombstone stands). The receiver's
  `DeleteForMe` for its own other devices waits for the
  self-conversation (Step 86).
- *No mentions / quote inside the wrapper* on send; receive tolerates
  both (the shared text tail handles them).
- *Per-row timer tasks* are unbounded in count (one per live
  ephemeral). Fine for the OTP shape; a room full of timed messages
  would benefit from a single self-rescheduling timer.
- *CLI mention parsing is skipped* in `--ephemeral` mode instead of
  rejected — documented, not enforced.

**Surprises.** The plan's open timer question was already answered by
the orientation notes (viewport vs delivery) — deciding it before the
first test kept the tests honest (`received_at_ms + ttl`, no read
receipts). Clippy pedantic rejects `Duration::from_secs(365*24*60*60)`
in favour of `from_hours`. The record-column churn (LEARNINGS §5.25).

**Smoke.** See the Step-89 report / observations §74: scripted
kupferdraht→kupferdraht round-trip (incl. the V1b delete arriving at the
sender) + kupferdraht→webapp delivery; the webapp countdown + vanish and
the webapp→kupferdraht direction are the eyes-on checklist that gates the
merge.

