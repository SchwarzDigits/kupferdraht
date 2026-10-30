# Step 25 — Message delivery status (Failed + Delivered)

> **Status:** merged. Closes the message-status gap from the POC statement of
> work: outbound messages need a **Failed** state (a send that fails today
> leaves no trace) and a **Delivered** state (Wire `Confirmation` type
> `DELIVERED`, currently decoded but dropped). READ receipts already ship
> (Step 6-II-f). Local `Failed` state on send error + `Confirmation(DELIVERED)`
> GenericMessage (1:1 auto-ack); `delivered_to` on timeline items. No new
> backend surface. `fb30f5f` (#12). Archived from PLAN.md.
>
> **Sub-steps:**
> - **25a** — Failed status (local, send-failure). Branch `step/25a-message-failed-status`.
> - **25b** — Delivered status, both directions. Branch `step/25b-message-delivered-status`.
>
> **Commit prefix:** `feat(step-25a): …`, `feat(step-25b): …`.

---

## Why

`DeliveryStatus` today has only `Submitted` (outbound, awaiting the Wire echo)
and `Received` (inbound). Two states the POC requires are missing:

- **Failed** — `persist_outgoing_text` runs only after a successful upload, so a
  send that fails (`encrypt_and_upload` errors) leaves **no row at all**: the
  message silently vanishes from the UI instead of showing a retry-able failure.
- **Delivered** — Wire's `Confirmation` carries `DELIVERED = 0` and `READ = 1`
  (`proto/generic_message.proto:73`). Kupferdraht sends READ receipts but deliberately
  descoped DELIVERED (decoded, then dropped at `sync/dispatch.rs:83`). Reopening
  it is approved.

## matrix-rust-sdk analog

matrix-rust-sdk splits these two concerns the same way Kupferdraht does: a **local**
scalar send-state (sending / sent / failed) on the outbound item, and **read
receipts as a separate per-user list** the app aggregates for display. Step 25
keeps that shape — `DeliveryStatus` stays the local scalar (`+ Failed`), and
`Delivered` is modeled as a receipt **list** (`delivered_to`), mirroring the
existing `read_by`.

## Modeling decisions

- **`delivered_to: Vec<String>`** (a list of confirmer user ids), mirroring
  `read_by` — *not* a scalar `Delivered` status. The confirmer identity is the
  MLS-authenticated sender of the `Confirmation` (already extracted as
  `sender_user_id` and surfaced for READ). Independent lists sidestep any
  "a late DELIVERED must not overwrite a READ" ordering guard.
- **One `Failed`, no `FAILED_REMOTELY`.** A partial "sent to some backends but
  not the remote one" state is federation-specific, and federation is out of
  scope (ADR / CLAUDE.md). MLS is binary: the backend accepts the single group
  ciphertext or it doesn't. The *cause* lives in the returned `KupferdrahtError`
  (`Network` vs `BackendRejected` …), not in the status.
- **Delivery auto-send is 1:1 only.** In a group of N members, every message
  would trigger N delivery confirmations sent and N received — N² traffic for
  little value ("delivered to whom?"). DMs get the `✓✓`; groups use the
  read-by list. The **receive** side applies an incoming `DELIVERED` in *any*
  room type (harmless if some client sends one); only the auto-emit is gated on
  `RoomType::DirectMessage`.
- **Delivery is ungated by the read-receipt setting.** `ReceiptMode::Off`
  suppresses outbound READ receipts (privacy); it does **not** suppress
  DELIVERED. Wire's `expects_read_confirmation` flag governs READ only — there
  is no delivery-confirmation opt-out.

## 25a — Failed

- `DeliveryStatus::Failed` in `timeline/items.rs` (+ `"failed"` in
  `as_db_str`/`from_db_str`). The `V6` `timeline_events.delivery_status` column
  already documents `"failed"` — **no migration**, and `DeliveryStatus` rides the
  existing `uniffi::Enum`, so **no FFI struct change**.
- On the outbound **new-message** sends (`send_text`, `send_reply`,
  `send_text_with_mentions`, `send_file`): when `encrypt_and_upload` errors,
  persist the row with `Failed` (the `message_id` + plaintext are already in
  hand), emit `TimelineDiff::Append`, then propagate the `Err`. A shared helper
  hosts this so the call sites don't repeat it. `edit` / `redact` /
  `toggle_reaction` / confirmations are out of scope (they mutate existing items
  or aren't user-tracked bubbles).
- **No retry** on the Kupferdraht side — the caller re-sends.

## 25b — Delivered (both directions)

- **Model + store:** `delivered_to: Vec<String>` on `TimelineItem` (+ the FFI
  `TimelineItem`); a `delivery_receipts` table (migration `V22`, mirroring
  `V15__step_6_ii_f_read_receipts.sql`); `Store::apply_delivery_receipts(...)`
  mirroring `apply_read_receipts`; aggregate `delivered_to` in the item-assembly
  query alongside `read_by`.
- **Receive:** in `dispatch_inbound_confirmation`, handle `type == DELIVERED`
  (currently dropped) → resolve canonical ids → `apply_delivery_receipts` → emit
  `UpdateAt`. READ and DELIVERED share the id-resolution + emit; only the apply
  call and the populated field differ.
- **Send / auto-emit:** a `build_delivered_confirmation_generic_message` +
  `Client::send_delivered_confirmation` mirroring the READ pair (no local
  self-apply — `delivered_to` is the sender's view). Hooked into the inbound
  text/asset dispatch after a successful persist (`inserted == true`), **only for
  `RoomType::DirectMessage`**, ungated. Self-echoes and non-content messages are
  excluded naturally, so there is no confirmation loop.
- **Batching:** accumulate the per-DM message ids during a sync iteration and
  flush one delivery confirmation per room at the end of the batch, rather than
  one MLS send per message (per-message is an acceptable fallback if the sync
  structure makes batching awkward — log the choice).

## Consumer mapping (natrium aggregates the scalar)

Kupferdraht exposes raw facts; the consumer composes the bubble. natrium owns a scalar
`MessageStatus` and computes it with an "any" rule:

```
FAILED      if delivery_status == Failed
READ        else if read_by is non-empty
DELIVERED   else if delivered_to is non-empty
SENT        else            (Submitted = uploaded to Wire)
```

Exact for 1:1; in groups `DELIVERED` does not appear (delivery is DM-only) and
`READ` means "someone read". Kupferdraht does **not** compute this scalar.

## Dependencies

- Step 4 (timeline send path, `DeliveryStatus`, `persist_outgoing_text`).
- Step 6-II-f (read receipts — the pattern `delivered_to` mirrors).
- Step 10f (`ReceiptMode` — the gate that delivery deliberately does **not** use).

## Tests

- 25a: a send whose upload fails → row persisted as `Failed` + `Append` emitted +
  `Err` returned; happy path still `Submitted`.
- 25b: inbound `DELIVERED` confirmation → `delivered_to` populated + `UpdateAt`;
  inbound text in a DM → a delivery confirmation is sent (mock-asserted), in a
  group → not sent; delivery still sent when `ReceiptMode::Off` while READ is
  suppressed.

## Verification

- Gate: `cargo fmt --check`, build 0 warnings, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny`.
- Binding smoke: Kotlin + Swift see `DeliveryStatus.Failed` and
  `TimelineItem.deliveredBy`.
- Live (manual): 1:1 send → recipient online → sender sees Delivered; offline
  send → Failed.

## Out of scope

Retry of failed messages; delivery confirmations in groups; a separate
"Sent"/server-ack state distinct from `Submitted` (Submitted already means
uploaded to Wire).
