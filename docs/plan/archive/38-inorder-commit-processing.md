# Step 38 — In-order commit processing on the receive path

> **Status:** MERGED to `main` — `73653e0`, PR #32 (squash-merge, no
> separate merge commit), 2026-06-18. Scope: stops inbound message loss
> during MLS epoch drift by typing the out-of-order case and healing the
> room instead of silently dropping the event; companion to Step 37, which
> fixed the **send** side. Live verification pending (as of 2026-07-16) —
> the file's own TDD section states real out-of-order behaviour "needs a
> two-client, two-device harness that isn't available here — it must be
> validated on real devices", and no later record confirms that happened.
>
> **Sub-steps:** none.
>
> **Historical names:**
> - "Step 38" — current.
>
> **Branch / commit prefix:** `step/38-*`, `fix(step-38): …`.

---

## Step 38 — In-order commit processing on the receive path

Step 37 made **sending** recover from a stale epoch. This step fixes the mirror
problem on **receive**: an inbound message whose epoch is ahead of the local
group (its advancing commit was missed) must not be silently discarded.

### The bug

`SyncService::dispatch_notification` walks the events in a notification frame,
dispatches each, and then advances the persisted `last_event_id` cursor. The old
code folded **every** per-event error into `on_sync_error` and advanced the
cursor regardless. So when a `conversation.mls-message-add` failed to decrypt
because the local group sat at an older epoch — openmls
`ProcessMessageError::ValidationError(ValidationError::WrongEpoch)` — the event
was logged, skipped, and the cursor moved past it. The message was lost for good:
the next poll never re-fetches it.

This is exactly the reported symptom — a colleague's messages, sent while the
local epoch had drifted, never arrived even though the connection later
recovered.

### Fix

Three parts, all in one step:

1. **Type the error (recoverable vs. permanent).**
   `process_inbound_message` (`encryption/messages.rs`) now routes
   `process_message` failures through `classify_process_error`, which maps
   `ValidationError::WrongEpoch` to a new `KupferdrahtError::MlsOutOfOrder` and leaves
   everything else as the opaque `Generic`. The receive drain can now tell a
   recoverable epoch gap apart from a genuinely broken message.

2. **Don't advance past a recoverable failure — heal it.**
   `dispatch_notification` matches `MlsOutOfOrder` distinctly: it calls
   `recover_room`, which resyncs the room to the current epoch via
   `Client::resync_room` so subsequent messages in that room decrypt. The resync
   is deduped per drain through a `resynced: HashSet<String>` field (cleared at
   the top of each `drain_pending`): a stale backlog of many old-epoch frames for
   one room triggers **one** resync, not one per frame. Other (permanent)
   per-event errors are still logged + skipped so a single bad message can't
   wedge the stream.

   The cursor still advances after each frame. Re-fetching the same frame is
   deterministic — holding the cursor would only re-fail at the same spot and
   stall the stream — so the heal (resync) is what closes the gap, not a retry of
   the dropped event.

3. **Light in-place catch-up.**
   The cheap catch-up primitive already exists and is now exercised first:
   `process_inbound_message` merges a `StagedCommitMessage` inline
   (`merge_staged_commit`) before the application messages that follow it in the
   same frame. So an advancing commit delivered in-order is applied in place — no
   resync needed. `resync_room` is the heavier fallback, reached only when the
   commit was missed entirely (a gap this frame can't close).

### Why resync as the fallback (and its cost)

`resync_room` is heavy — it discards the local group and does a full
external-commit re-join, which is forward-secrecy-lossy and costs a round-trip.
It stays the fallback only for an unfillable gap. With the receive path now
applying in-order commits and healing on drift, the send-side `resync_room` from
Step 37 should rarely fire. A lighter "fetch + process just the missing commits"
primitive would avoid the re-join entirely, but none exists yet; that is a future
optimisation.

A deliberate non-goal: no send-side drain was added for catch-up. Draining from
the send path would race the sync guard (`KupferdrahtError::SyncAlreadyRunning`); the
receive-path heal plus Step 37's send-path resync cover the cases.

### Conflict-free with Step 36

This touches `error.rs`, `encryption/messages.rs`, and `sync/service.rs` — all
disjoint from the unmerged Step 36 (`asset_send.rs` / `asset_crypto.rs` /
`ffi/timeline.rs` / `timeline/mod.rs`). It builds on Step 37's `error.rs` change
(the `MlsStaleMessage` variant); the new `MlsOutOfOrder` variant is added
adjacent to it. No public signatures change, so no caller file is modified and
the step merges in any order relative to Step 36.

### TDD

`encryption/messages.rs` (unit):
- `classify_process_error` maps `WrongEpoch` → `MlsOutOfOrder`.
- any other `ValidationError` stays `Generic`.

Real out-of-order behaviour (cursor held vs. advanced, resync dedup per drain)
needs a two-client, two-device harness that isn't available here — it must be
validated on real devices: send while a peer's epoch has drifted and confirm the
message arrives after the drain heals the room.

> **Note (merge with Step 36):** an Android `send_file` tempdir fix was briefly
> carried on this branch, but Step 36 (#31) landed the same fix independently as
> `asset_staging_dir()`. The duplicate was dropped when `main` merged in; the
> Android fix now lives in Step 36, not here.
