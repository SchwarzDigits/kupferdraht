# ADR-0028: Authorize inbound edits and deletes by the original sender, with Wire's delete exceptions

**Status:** Accepted
**Date:** 2026-09-26
**Deciders:** Markus Troßbach

## Context

`MessageEdit` and `MessageDelete` are applied by target message id and
room only (`dispatch_inbound_edit` / `dispatch_inbound_delete` receive no
sender). Any member — through an honest backend — can rewrite or redact
another member's message, including task carriers (ADR-0014) and case
snapshots (ADR-0018). Found in the production-readiness review of
2026-09-25 (blocker 2).

Official Wire clients bind both operations to the original sender: an
edit applies only when its sender's user is the target's sender (any
device of that user). A delete applies when its sender's user is the
target's sender, or — for an **ephemeral** target only — when its sender
is the self user (another own device) or the target is the self user's
own message. Both ephemeral exceptions exist because Wire enforces
ephemeral expiry on the receiving side: the receiver deletes the expired
message for the sender (observations §74) — Kupferdraht's Step-89 receivers
do the same, and the receiver's other own devices sync that expiry. The
official webapp ignores a self-sent delete of another member's ordinary
message (observations §91, black-box test 2026-09-28). Wire has no admin
moderation delete. Constraints from prior decisions: an authorization by
sender is only meaningful with an authenticated sender (ADR-0027).

## Options Considered

### Option H: Wire's rule

Edit by the original sender's user; delete by the original sender's user
or — for an ephemeral target — by the self user or of the self user's own
message; no admin override.

**Pros:**
- Wire parity.
- Keeps Step 89's receiver-side expiry working on the sender's devices and
  on the receiver's own other devices.

**Cons:**
- Any member can hasten the deletion of the self user's own ephemeral
  messages on the self user's devices.

### Option I: Additionally let conversation admins delete others' messages

**Pros:**
- Moderation.

**Cons:**
- Diverges from Wire; official clients would not mirror the deletion.

### Option J: Any member may delete any ephemeral message

**Pros:**
- Simplest ephemeral exception.

**Cons:**
- Lets a third party hasten the deletion of someone else's ephemeral
  message on every device; not Wire behaviour — each receiver enforces its
  own timer.

## Decision

Option H. Both operations require the target in the same room.

1. **Edit:** applies only when the edit's (ADR-0027) sender user id equals
   the target row's sender user id — any device of that user.
2. **Delete:** applies when (a) the delete's sender user is the target's
   sender, or (b) the delete's sender is the self user (another own
   device) and the target is **ephemeral** — the own-device sync of a
   receiver-side expiry (amended 2026-09-28, observations §91), or (c)
   the target is the self user's own ephemeral message (from any member).
   A third party's delete of someone else's ephemeral message is
   rejected, as is a self-sent delete of another member's ordinary
   message.
3. No admin override. A rejected edit or delete is dropped with a
   `warn!` and has no effect on the row, the timeline diffs, or the task /
   case folds.

## Consequences

### Positive

- No member can rewrite or redact another member's message, task carrier
  or case snapshot.
- Wire parity; Step 89's receiver-side expiry keeps working.

### Negative

- Any member can hasten the deletion of the self user's own ephemeral
  messages on the self user's devices — inherent to Wire's
  receiver-enforced expiry.
- For senders attributed by the envelope (X.509 leaves, ADR-0027 decision
  2) the authorization inherits the envelope's weakness until the X.509
  follow-up.
- A rejected edit or delete is visible only in the log.

### Neutral

- The public `Store` trait is unchanged; the check sits in the dispatch
  layer.
- Task updates stay open to any member (ADR-0014) — they are new messages,
  not edits.

## Implementation Notes

- `sync/dispatch/edit.rs` / `delete.rs` receive the ADR-0027 sender,
  resolve the target in the same room with
  `Store::load_timeline_event_in_room` (message id or current edit id),
  and enforce the rule in `sync/dispatch/authorize.rs` before
  `apply_message_edit` / `apply_message_delete`.

## References

- Builds on ADR-0027 (the authenticated sender the rule checks).
- Related: ADR-0014 (the carrier-delete interaction is now reachable only
  by the carrier's sender or the self user); ADR-0018 (case snapshots
  likewise); observations §74 and Step 89 (receiver-enforced ephemeral
  expiry).

## Revision History

- 2026-09-26: Created
- 2026-09-26: Sending side added in Step 95b (same rule, current truth — not a new decision): `Timeline::edit` / `Timeline::redact` refuse another user's message with `InvalidInput` before uploading, so the caller's local row never diverges from what every receiver keeps. Any device of the self user counts as own; a target unknown locally goes out unchanged. The receiver-side ephemeral expiry (`upload_message_delete`) is unaffected. Implementation note corrected: the target lookup is room-scoped (`Store::load_timeline_event_in_room`, a new trait method with a default body — non-breaking), because timeline rows are keyed `(message_id, room_id)` and the room-agnostic `load_timeline_event` could resolve another room's row.
- 2026-09-28: Clause (b) of the delete rule narrowed (review question on PR #56; Markus): a delete from the self user (another own device) applies to another member's message only when that message is **ephemeral** — the own-device sync of a receiver-side expiry (observations §74). Observed behaviour, not derived: the official webapp ignores a self-sent delete of another member's ordinary message (observations §91), so "self user — any target" was not Wire's behaviour. (a) and (c) are unchanged; the effect of (b) was always confined to the self user's own devices.
- 2026-09-28: Context, Option H and Decision 2(b) amended to the narrowed clause (same decision as the entry above, current truth — not a supersession); Implementation Notes now name the room-scoped lookup and the `authorize.rs` home the 2026-09-26 entry corrected. Review of PR #62 (Volker): the body had kept "self user — any target" while `rules/0028` and the code carried the narrowing, and the ADR wins over its rule.

