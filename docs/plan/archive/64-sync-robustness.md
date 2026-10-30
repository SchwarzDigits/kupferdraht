# Step 64 — Sync robustness: dedup, 404 recovery, generic-error healing

> **Status:** MERGED to `main` — `69f80ab` (+ narrowing in `469f543`),
> fast-forward 2026-08-14; live-smoked 2026-08-14 (42-notification
> real-queue drain; 404/dedup paths httpmock-pinned). Originally
> implemented on `chore/review-triage` (2026-08-13) —
> branch-per-step waived by Volker for the review-fix series ("everything
> on the same branch"); commits stay per-step
> (`feat(step-64): …`). 3/3 contract tests green; `resync_room` also
> made tolerant of a missing local group (nothing to discard → rejoin
> directly). No new public API → no CLI command, no FFI mirror; the
> behavior rides the existing `sync*` surface. Smoke: the happy path
> rides every routine sync against the instance; the 404/dedup paths
> are httpmock-pinned. From the external code review (triaged in
> `open.md` §66; findings S1, S7, S8, S13, all verified against the
> source at `f17abbd`).
>
> **Historical names:**
> - "Step 64" — proposed 2026-08-13.
>
> **Branch / commit prefix:** `chore/review-triage` (waiver, see
> Status), `feat(step-64): …`.

---

## Goal

Close the three verified event-loss/robustness gaps in the receive
path, and align the docs with the actual cursor semantics:

1. **Pull-path notification dedup (S8).** The push loop dedups
   redeliveries against the persisted `last_event_id`
   (`sync/push.rs::dispatch_deduped`); `drain_pending` does not — a
   crash between dispatch and cursor persist double-processes the
   frame on the next pull (duplicate `on_message` callbacks, replayed
   MLS decrypts that can trigger spurious room recoveries). Mirror
   the push dedup in the drain.
2. **Stale-cursor 404 recovery (S13).**
   `wire/http/notifications.rs` documents the fallback ("caller
   should fall back to a marker-less fetch or a cursor reset") and
   backend-spec §11.4 mandates it, but `drain_pending` just
   propagates the 404 — after a server-side queue purge, sync is
   wedged until `sync_state` is deleted manually. Implement: on 404,
   reset `last_event_id`, resync, continue.
3. **Generic-error healing (S1 residual) — narrowed after the
   review re-check:** recovery fires only on a positive
   broken-local-state signal (`Storage` / `InvalidInput`); a garbage
   frame in a healthy group surfaces as `Encoding`/`Generic` (the
   group loads before the ciphertext parses) and is skipped — healing
   on every class would let any member trigger discard+rejoin epoch
   churn once per drain by posting garbage. Original scope:** Per-event dispatch
   failures advance the cursor by design (documented in
   `dispatch_notification`) — but only `MlsOutOfOrder` triggers
   `recover_room`. A commit that fails with any other error class
   (storage hiccup, transient decode) is skipped without healing;
   the room then heals only when the *next* message trips
   `MlsOutOfOrder`. Trigger recovery for failed
   `conversation.mls-message-add` dispatches regardless of error
   class (dedup via the existing per-cycle `resynced` set).
4. **Docs (S7).** `api-mapping-matrix-rust-sdk.md` row for
   `MlsOutOfOrder` claims the drain "holds the cursor and retries"
   — it heals, it never retries. Fix the row and the ambiguous
   `error.rs` message ("catch up and retry").

## Non-goals

- A persistent retry queue for failed events (design discussion
  first — the cursor-always-advances model is deliberate and
  documented; this step narrows its blast radius, it does not
  replace it).
- Reordering the push loop's dispatch→ack sequence (Step 62's
  invariant stands; a failed dispatch on push is acked by design —
  after this step the room recovery covers it).

## TDD candidates

- drain: redelivered frame (same id) is skipped, cursor intact.
- drain: 404 on `since` resets the cursor and the drain recovers in
  the same call (httpmock: 404 first, marker-less fetch second).
- dispatch: a failing `mls-message-add` (non-`MlsOutOfOrder` error)
  triggers exactly one `recover_room` for the room.
- api-mapping row + error message corrected (review-only).
