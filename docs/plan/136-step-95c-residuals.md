# Step 136 — Step 95c residuals: lock-poison recovery, payload caps, function-length debt

**Status: commissioned 2026-11-02** (transferred from open.md §70, which
dissolves; the review of the external security scan of 2026-11-02 met the
second item as its finding L5 and the first as a finding the scan's judge
wrongly rejected). Three residuals Step 95c
(`docs/plan/archive/95-inbound-message-integrity.md`) left open on
2026-09-26, the first two deliberately.

## Problem

1. **Lock poison cascades.** 22 `.expect("… poisoned")` today on in-memory
   cache locks — the rooms cache, event-handler and listener registries,
   typing state (`client/{membership,rooms,sync,timeline_read,typing}.rs`,
   `sync/service.rs`). They fire only after another panic while the lock
   was held — no foreign data reaches them — but they turn one defect into
   a cascade: the first panic poisons the lock, every later reader panics
   too, and a sync loop or an FFI call dies on a lock that guards data the
   next pass would replace wholesale. `lock_mls`
   (`encryption/mls_lock.rs:23`) already recovers the MLS mutex with
   `clear_poison` + `into_inner` and a `warn!`.
2. **No caps on Kupferdraht's own payloads.** `task_payload_is_valid` and
   `case_payload_is_valid` (`sync/dispatch/{task,case}.rs`) check one
   required field each; 95c planned "required fields, length caps" and
   shipped the required fields only. A peer can send a very large title,
   description or `extensions` map; it is JSON-serialised and stored twice
   (the row and the fold). Wire's `/mls/messages` cap (kilobytes,
   ADR-0011) bounds one message, not many. A cap per field is a protocol
   decision — what a receiver drops, what a sender refuses — so it needs
   an ADR before code.
3. **Function-length debt the step touched.** Functions already over the
   40-line budget before 95c grew by a few lines each — `run_push` (61),
   `ClientBuilder::login` (52), `create_room` (50), `delete_room` (46),
   `absorb_welcome` (44), `dispatch_inbound_ephemeral` (44), `upsert_task`
   (49) — and `start_sync` crossed it (41). Other pre-95 overlong functions
   in the same files (`dispatch_generic_message` 66, `rebind_access_token`
   68, `create_dm` 62) are older debt. The numbers are of 2026-09-26;
   phase 1 re-measures.

## Scope

1. One `pub(crate)` helper after `lock_mls`'s pattern (`clear_poison` +
   `into_inner` + `warn!`), for data that is replaced wholesale; every
   `.expect("… poisoned")` in the listed modules goes through it. A lock
   whose data is *not* replaced wholesale keeps its `expect` with a
   comment naming why — phase 1 lists them (none known today).
2. Phase 1: an ADR "cap the fields of Kupferdraht's own payloads" — a byte
   or element limit per field of `TaskPayload`, `CasePayload`,
   `TaskListMarker` and `extensions` (`task_id`, `title`, `description`,
   `case_ref`, `summary`, `assignees`, `links`, `tags`, `extensions`
   keys/values, …), the same constants on both sides: the receiver drops an
   over-limit payload like a malformed envelope (`warn!`), the sender
   refuses it with `InvalidInput` before any network; `rules/` file in the
   same commit. Then the caps in `validate_*` at the decode boundary and in
   the send path; `docs/wire/message-types.md` lists the limits.
3. The functions of Problem 3 that live in a module items 1 or 2 touch are
   split to ≤ 40 lines in this step; the others are re-measured and listed
   in the retrospective for the step that touches their module next
   (Conventions, "≤ 40 lines per function").

## Out of scope

- Caps for Wire's own content variants (`Text`, `Asset`, …): Wire's
  message cap and Wire's clients govern them, not Kupferdraht's protocol.
- Lock recovery for the MLS mutex (`lock_mls` has it) and for `std::sync`
  locks in `kupferdraht-cli`.
- A size cap on the whole inbound notification page (the backend bounds it).

## TDD sketch

- A test poisons a cache lock from a panicking thread; the next reader
  recovers, logs one `warn`, and sees the data the next refresh writes.
- An over-limit task or case payload is dropped and warned; a limit-sized
  one is accepted and folded; `Timeline::set_task` / `set_case` with an
  over-limit field fail `InvalidInput` before any request (httpmock sees
  none). A doc pin that `docs/wire/message-types.md` lists every limit the
  constants define.
- CLI: `task add --title <over limit>` is refused with the limit in the
  message; `task add` at the limit round-trips.
- Gate; smoke: one limit-sized task round-trips live between two devices
  (no backend surface changes, but the serialized size is what the live
  cap measures).
