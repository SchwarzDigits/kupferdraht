# Wire GenericMessage Message Types

Every piece of conversation content Kupferdraht sends or receives is one
`GenericMessage` Protobuf with a `content` oneof — Wire has no separate
message format per feature. This document is the human-readable catalogue
of Wire's **complete** standard vocabulary plus Kupferdraht's three extensions:
what each variant is, what Kupferdraht implements, and where the out-of-scope
line runs.

The authoritative schema for what Kupferdraht emits/decodes is
[`kupferdraht-core/proto/generic_message.proto`](../../kupferdraht-core/proto/generic_message.proto).
That file is a modified subset of Wire's schema (GPL-3.0-or-later): Wire's
notice and Kupferdraht's modification notice sit in its header, and the
comments are Kupferdraht's own.
This doc tracks it: a change to an emitted/decoded variant updates both in
the same commit (CLAUDE.md "Wire Message Types" + Freshness rule). The
Kupferdraht proto deliberately declares **only** the variants Kupferdraht emits or
decodes; every other Wire variant takes the unknown-variant path —
deserialize the envelope, log, discard, never skip at the envelope layer
(CLAUDE.md "Wire Message Types"). Wire's side of the catalogue was last
cross-checked against
[`wireapp/generic-message-proto`](https://github.com/wireapp/generic-message-proto)
master on 2026-09-24 (Step 83; first pass 2026-07-24).

## Envelope

| Field | Type | Notes |
|---|---|---|
| `message_id` | `required string` (1) | Client-generated random id (UUID). Drives Wire's delivery-receipt and edit/delete tracking; set on the sender side, retained on receipt, surfaced by the Wire UI for quote / reply / edit references. |
| `content` | `oneof` | Exactly one of the variants below. An **unknown** variant decodes to `None`: deserialize the envelope, log, discard — never skip at the envelope layer (CLAUDE.md "Wire Message Types"). |
| `unknown_strategy` | `optional UnknownStrategy` (25) | **Wire field, not declared by Kupferdraht** — sender's hint for how a receiver should treat a message it cannot render: `IGNORE` (0, default), `DISCARD_AND_WARN` (1), `WARN_USER_ALLOW_RETRY` (2). Notable as precedent: Wire itself grew an envelope field *outside* the `content` oneof — the same structural pattern as Kupferdraht's extensions below. |
| `task` | `optional TaskPayload` (262000) | **Kupferdraht extension** (Step 60a, ADR-0014) — deliberately OUTSIDE the oneof so it can ride alongside a `Text` fallback. Receive-side dispatch checks this field **before** the oneof. See "TaskPayload" below and `mls-deviations.md` W-10 for the field-number collision strategy. |
| `case` | `optional CasePayload` (262001) | **Kupferdraht extension** (Step 72, ADR-0018) — full case snapshot, never with a text fallback. See "CasePayload" below. |
| `task_list_marker` | `optional TaskListMarker` (262002) | **Kupferdraht extension** (Step 75, ADR-0020) — room-level task-list flag, never with a fallback. See "TaskListMarker" below. |

## The full variant catalogue

All `content` variants Wire's schema declares, in field-number order
(field 5 is unassigned), plus Kupferdraht's non-oneof extensions. Reading the
**Out of scope?** column: **yes** is a deliberate decision with a reason;
*no — open* means unimplemented today with nothing decided against it
(backlog reference where one exists); — means implemented, question moot.
Every variant Kupferdraht does not decode takes the unknown-variant path
described above — that path IS the "reserved" list; there is no separate
one.

| Variant | Field | Kupferdraht | Out of scope? |
|---|---|---|---|
| `Text` | 2 | emit + receive — optional `mentions` and a `quote` (reply) | — |
| `ImageAsset` | 3 | — | **yes** — deprecated by Wire, replaced by `Asset` (11) |
| `Knock` | 4 | emit + receive (Step 26) — nudge / "ping"; `hot_knock` marks a repeat | — |
| `LastRead` | 6 | — | no — open: cross-device read position; rides the self-conversation once Step 86 lands |
| `Cleared` | 7 | — | no — open: history-cleared marker synced via the self-conversation; no step planned |
| `External` | 8 | — | **yes** — Proteus-era large-payload indirection (payload AES-encrypted once, key fanned out per session); an MLS group produces a single ciphertext, the mechanism has no MLS use |
| `ClientAction` | 9 | — | **yes** — Proteus session reset; Kupferdraht never performs Proteus encryption (CLAUDE.md "What NOT to do") |
| `Calling` | 10 | — | **yes** — no calling, ever (charter) |
| `Asset` | 11 | emit (AES-CBC) + receive (Step 11, ADR-0011); inbound AES-GCM tolerated read-only as "unsupported asset" | — |
| `MessageHide` | 12 | — | no — open: hide-on-own-devices marker synced via the self-conversation; no step planned |
| `Location` | 13 | emit + receive (Step 26) — required `longitude`/`latitude`, optional `name`/`zoom` | — |
| `MessageDelete` | 14 | emit + receive — deletes a prior message by `message_id`; also the receiver-side expiry broadcast for `Ephemeral` (Step 89) | — |
| `MessageEdit` | 15 | emit + receive — **text-only** | partial — text edits in scope; `Composite`/`Multipart` edits follow fields 20/27 |
| `Confirmation` | 16 | READ emit (receipt-mode-gated) + receive; DELIVERED auto-ack emit (1:1 only) + receive (Steps 6-II-f, 25b) | — |
| `Reaction` | 17 | emit + receive — full-set toggle semantics | — |
| `Ephemeral` | 18 | emit (`Text` inner arm) + receive (`Text` inner arm; other arms tolerated-unknown) (Step 89) — TTL rides in the message, every client runs its own countdown | — |
| `Availability` | 19 | — | **yes** — manual presence status (available/away/busy); presence is deliberately out on privacy grounds (open.md "Presence" non-goal) |
| `Composite` | 20 | — | **yes (V1)** — text-plus-buttons / poll surface; revisit on consumer demand |
| `ButtonAction` | 21 | — | **yes (V1)** — a button press answering a `Composite`; follows field 20 |
| `ButtonActionConfirmation` | 22 | — | **yes (V1)** — sender's confirmation of a `ButtonAction`; follows field 20 |
| `DataTransfer` | 23 | — | **yes** — Wire-internal client-data sync channel (official clients ride it over the self-conversation) |
| `InCallEmoji` | 24 | — | **yes** — calling surface (charter) |
| `InCallHandRaise` | 26 | — | **yes** — calling surface (charter) |
| `Multipart` | 27 | — | **yes (V1)** — Wire's newer text-plus-attachments bundle (added upstream after 2026-07); decodes to the unknown-variant path; revisit with field 20 on consumer demand |
| `TaskPayload` | 262000 (non-oneof) | emit + receive + per-task LWW fold (Steps 60a–c, ADR-0014) | — (**Kupferdraht extension**, not Wire vocabulary) |
| `CasePayload` | 262001 (non-oneof) | emit + receive + LWW-by-revision fold, admin-gated core (Step 72, ADR-0018) | — (**Kupferdraht extension**) |
| `TaskListMarker` | 262002 (non-oneof) | emit + receive + LWW fold, any member (Step 75, ADR-0020) | — (**Kupferdraht extension**) |

## Sub-structure notes

These are the non-obvious semantics behind the implemented variants;
field-level detail lives in the proto.

**`Text` — mentions are UTF-16.** `Mention.start`/`length` are UTF-16
code-unit offsets into `Text.content` (matching Wire's iOS/Android/Web
clients). Kupferdraht converts its Rust UTF-8 byte positions to UTF-16 before
serializing. Mentions carry a `QualifiedUserId` (`id` + `domain`); Kupferdraht
emits the qualified id exclusively — legacy unqualified ids are unsafe in
federated rooms and are accepted on receive only.

**`Text` — quote digest.** `Quote.quoted_message_sha256` is declared
optional by Wire but Wire's clients refuse to render a reply as a quote
without it, so Kupferdraht computes and emits it. The digest is
`sha256( 0xFE 0xFF || body_utf16_be || timestamp_seconds_be )`, where the
timestamp is the parent's send-time in seconds as a big-endian i64
(observations.md §33).

**`Ephemeral` — the timer runs on every client, not the server.**
Wire's wrapper carries `expire_after_millis` (`required int64`) and an
inner `content` oneof (`Text = 2`, `ImageAsset = 3` deprecated,
`Knock = 4`, `Asset = 5`, `Location = 6`). Kupferdraht declares only the
`Text` arm in V1 (the one-time-code case):
outbound `Timeline::send_ephemeral_text(body, ttl)` emits it (no
mentions, no quote inside the wrapper; `ttl` is bounded to
1 s … 365 days, the range official clients accept); inbound
`Ephemeral(Text)` lands as an ordinary `Text` row / `MessageBody::Text`
decorated with `expires_at_ms`, and every other inner arm decodes to
`content = None` — logged and discarded at the content layer, never at
the envelope layer. **Timer start (V1 = dispatch/send time).**
Live-observed on wire-webapp (observations §74): an official
*receiver* starts the countdown only when the message scrolls into the
viewport, the *sender* on its own copy once the send hits the
backend. A headless client has no viewport, so Kupferdraht starts the
receive-side timer at dispatch (`received_at_ms + ttl`) — never later
than an official client, i.e. it may delete sooner but never shows a
message longer. The send side matches the official sender
(`sent_at_ms + ttl` once the upload succeeded; a `Failed` row gets no
timer). **Expiry = local tombstone + receiver-side broadcast.** The
elapsed row is tombstoned exactly like a `MessageDelete` (body
cleared, `redacted_at_ms` set; `expires_at_ms` stays so an expiry
remains distinguishable) and surfaces as `TimelineDiff::UpdateAt`
with `TimelineItemKind::Redacted`. Official clients split the
enforcement by role (observations §74): the *receiver* broadcasts the
expiry as a `MessageDelete` for the message, the *sender* only marks
its own copy and waits for exactly that delete to purge it. Kupferdraht
mirrors this: an expired **inbound** row is followed by one
`MessageDelete` into the room (best-effort — a rejected upload is
logged, the local tombstone stands); an expired **own** row stays
local-only. Not mirrored yet: the `DeleteForMe` official receivers
send into their self-conversation for their other devices (Step 86).

**`Confirmation` — READ and DELIVERED are asymmetric.** READ is emitted
by `Timeline::mark_as_read`, gated by the server-administered receipt
mode (`receipt_mode()` Off → no send), and self-applied locally so
`read_by` includes the reader immediately. DELIVERED is the recipient's
auto-acknowledgement (Step 25b): emitted per *fresh* inbound content
message in 1:1 rooms only (group delivery would be N² traffic for little
value), **ungated** by the receipt-mode setting, and best-effort — a
failed ack never breaks receiving; redeliveries are not re-acked. Inbound
confirmations of either type land in sibling tables (`read_receipts` /
`delivery_receipts`) and overlay `read_by` / `delivered_to` on the
message; DELIVERED has no local self-apply — `delivered_to` is the
*sender's* view of who confirmed.

**`Reaction` — full-set semantics.** `emoji` is the user's *current
complete* set of reactions on the parent after the toggle, comma-separated;
an empty string clears all. Receivers split on `,`, trim, drop blanks, and
replace the sender's full set. `message_id` is the original message (or, on
Wire-Web, the latest edit-event-id in the edit chain).

**`Asset` — two channels.** The `Asset` message travels through MLS like
any other content variant, but the asset *bytes* travel **off-MLS** through
Wire's `/v14/assets` REST endpoint, AES-256-CBC encrypted with a one-time
random key (ADR-0011). The message carries only the reference:
`Original` (mime/size/name, plus `ImageMetaData` for images) and a
`RemoteData` status (`otr_key`, `sha256`, asset locator, `encryption`).
Kupferdraht emits only `AES_CBC`; inbound `AES_GCM` is decoded and surfaced as
an "unsupported asset" timeline item without panic (receive-side toleration
only — ADR-0011). `Preview`, video/audio metadata, and `legal_hold_status`
are decoded-but-not-consumed in V1.

**`TaskPayload` — two encoding modes (ADR-0014).** Mode c (normal
chats): the message sets field 262000 **and** the `Text` oneof variant as
human-readable fallback ("☐ <title> — @<handle>", assignee mention with
UTF-16 offsets) — official Wire clients render the text, Kupferdraht parses
the payload. Mode a (dedicated task-list conversations): field 262000
only, oneof unset — official clients treat the message like an unknown
variant and ignore it. Unknown `status` enum values decode as `OPEN`
(the value is pinned to 0 so prost's accessor default applies): the
task stays visible and the next full-state update heals it. `task_id`
is the stable identity — deliberately NOT the envelope `message_id`,
which edit chains mutate (see `Reaction` above).

**`CasePayload` — full-snapshot case state (ADR-0018, field 262001).**
The second Kupferdraht sibling outside the `content` oneof, **never** with
a text fallback: plain Wire clients discard the unknown payload
(envelope rule) instead of drowning in fallback noise. Carries the
full typed case core (`revision`, `case_ref`, `external_ref`, title,
state enum, assignee, dates, priority, `authority_ref`) plus a
display-only `extensions` map; receivers fold LWW by `revision`,
core fields only from senders who are conversation admins at receive
time. *(Recorded late — Step 72 shipped this without updating this
doc; caught in the Step-75 pass.)*

**`TaskListMarker` — room-level task-list flag (ADR-0020, field
262002).** The third Kupferdraht sibling outside the oneof, never with a
fallback. Full marker state (`marked: bool`), folded last-writer-wins
in event order like tasks; **any member** sets or clears it — no
admin gate, no revision. Declares a room a dedicated task list
(`Room::is_task_list()`); task sends in a marked room default to
mode a, and the inviter re-broadcasts marker + every task's folded
state to late joiners (the ADR-0020 MUST).

## Where handled in code

Outbound variants are built in the per-interaction send files under
`timeline/` (text, reaction, confirmation, delete, knock, location, asset,
task, task-list marker, ephemeral); inbound variants are decoded and routed by the `sync/dispatch/`
handlers (one per interaction; `dispatch/ephemeral.rs` unwraps the
field-18 wrapper and reuses the text tail). See `docs/ARCHITECTURE.md` §3 (module map) and §5
(data flows). All sends funnel through one chokepoint
(`Client::encrypt_and_upload`), which is what gives every variant epoch
self-healing for free.
