# Step 106 — Heal only on a confirmed gap; leave a group only on the backend's word

**Status: MERGED 2026-09-29** — commissioned 2026-09-29 (Markus — finding
H4 of the production-readiness review, `open.md` §71). `ebb083d`
(ADR-0034 + rule), `9b01046`. Merged to `main` as PR #77 (`01c0141`); CI
green. Smoke 2026-09-29 against schwarz-dev-wire (release CLI). Scope
shipped: a later-epoch message heals only when the backend confirms the
gap; storage errors retry instead of healing; only the backend's Remove
of our leaf drops the group. Retrospective: `docs/LEARNINGS.md` §5.46.
Open question kept in `open.md` §71 (whether the backend refuses
future-epoch messages). Archived with this header.

## Problem (verified in code 2026-09-29)

- A message from a later epoch than the local group (not behind an own
  pending commit) fails as `MlsOutOfOrder` and queues `HealRoom`:
  discard + rejoin by external commit. The epoch of a message the
  client cannot decrypt is unauthenticated.
- A `Storage` error while processing an `mls-message-add` is skipped and
  queues `HealRoom` (the Step 64 "torn merge" signal — impossible since
  Step 97c).
- `classify_pending_removes` returns `Own` for a Remove of our leaf from
  any sender; the removal flush then discards the local group.

## Decisions (ADR-0034, 2026-09-29, Markus)

- **D1** — before healing for a later-epoch message, read the
  conversation's current epoch from the backend (`POST
  /v14/conversations/list`, the one id); heal only when it is ahead of
  the local group, else skip and report. A missing local group heals as
  before.
- **D2** — a `Storage` error while processing an MLS message is a
  persist-class failure (ADR-0032): the notification aborts, rolls back
  and is retried; it never heals.
- **D3** — only an external-sender Remove of our own leaf discards the
  local group; a member's Remove of our leaf is left alone (we cannot
  commit it; another member does). Committing all pending proposals for
  a peer's Remove is unchanged.

## TDD

- `tests/step_106_narrow_healing.rs`: a peer advances the epoch without
  the client seeing the commit, then sends a message. The backend's
  conversation record at the local epoch → no rejoin (no `groupinfo`
  request); at the peer's epoch → the room heals (pin).
- `sync/service.rs`: a `Storage` failure while processing an MLS message
  is a persist failure, and queues no heal.
- `encryption/external_removal.rs`: a member's Remove proposal for our
  own leaf yields `Nothing`, not `SelfRemoved`.

**As built (2026-09-29).**
- `client/epoch_gap.rs`: `Client::backend_epoch_ahead` — local group
  epoch vs. the conversation record's (`list_conversations` for the one
  id).
- `sync/follow_ups.rs`: `FollowUp::HealRoomIfBehind` (a later epoch) next
  to `HealRoom` (group missing); `heal_if_backend_ahead` resyncs only on
  `true`, and reports a skipped message or a failed lookup through
  `notify_error`.
- `sync/service.rs`: `EventError::from_mls_step` (`Storage` →
  `Persist`); the Step 64 heal filter keeps `InvalidInput` only;
  `queue_room_recovery` takes the heal follow-up (deduped per room).
- `encryption/external_removal.rs`: `classify_pending_removes` reads
  `QueuedProposal::sender()`; a member's Remove of our leaf → `None`
  (nothing dropped, nothing committed while it is queued).
- Tests: 4 red → green, 1 pin (a confirmed gap heals).
- Smoke 2026-09-29 (schwarz-dev-wire, release CLI): user 1's smoke device
  and a fresh device of user 2 (joined the 1:1 by external commit, purged
  afterwards); a text from A reached B, both syncs without a heal,
  warning or error. A real epoch gap cannot be forced live (the
  notification queue delivers in order) — D1 is covered by the
  integration tests.
