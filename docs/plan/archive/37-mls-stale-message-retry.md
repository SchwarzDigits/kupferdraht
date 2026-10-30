# Step 37 — Self-heal on `mls-stale-message`

> **Status:** MERGED to `main` — `ec320d5`, PR #30 (squash-merge, no
> separate merge commit), 2026-06-18. Scope: self-heal message sends on a
> stale MLS epoch (`409 mls-stale-message`) instead of failing outright;
> surfaced by the natrium-ion send analysis. No dated live-smoke record in
> this file — its own TDD section describes a store-attached integration
> harness test, not a logged live-Wire run.
>
> **Sub-steps:** none.
>
> **Historical names:**
> - "Step 37" — current.
>
> **Branch / commit prefix:** `step/37-*`, `fix(step-37): …`.

---

## Step 37 — Self-heal on `mls-stale-message`

When a send targets a stale MLS epoch, recover automatically (resync the room to
the current epoch, retry once) instead of surfacing the error.

### The bug

Sending any message (`send_text`, `send_file`, `send_reply`, `toggle_reaction`,
`send_knock`, `send_location`, `redact`, `edit`, read/delivered confirmations)
encrypts at the group's **local** epoch and POSTs `/v14/mls/messages`. If the
conversation epoch advanced on the server (a commit the client hasn't processed)
Wire rejects with `409 {"label":"mls-stale-message"}`. The send path
(`Client::encrypt_and_upload`) propagates that verbatim — no resync, no retry —
so every send fails until the client happens to catch up. Today the local epoch
can drift behind silently (the pull-sync does not process general inbound
commits), so this is hit routinely after another member commits.

The recovery primitive already exists — `Client::resync_room` (discard the local
group, re-join via external commit → `server_epoch + 1`) — and the codebase even
notes "the `409 mls-stale-message` retry live in the sync/client layer"
(`external_removal.rs`). It just was never wired into the send path.

### Fix

1. **Typed error.** `reject_from_response` maps a `409` carrying the
   `mls-stale-message` label to a new `KupferdrahtError::MlsStaleMessage` (instead of
   the opaque `BackendRejected`), so the send path can match it precisely.
2. **Self-heal in the one shared chokepoint.** `Client::encrypt_and_upload` — the
   funnel **every** message send goes through — splits into a single
   `encrypt_and_send_once` attempt wrapped by a retry: on `MlsStaleMessage`,
   resolve the `room_id` from the `group_id` (scan the room cache),
   `resync_room` (advance the epoch), then re-encrypt + resend **once**. A second
   stale (epoch raced again mid-resync) or any other error propagates.

Because the fix lives in `encrypt_and_upload`, all message types inherit it with
no per-caller change. The `group_id` is stable across the resync (same group,
new epoch), so re-encrypting and resending is valid.

### Why `resync_room` (and its cost)

`resync_room` is heavy — it discards the local group and does a full
external-commit re-join (an extra round-trip + its own commit bundle). A lighter
"fetch + process the pending group commits to advance the epoch in place" would
be cheaper, but no such primitive exists yet; `resync_room` is the available,
correct recovery. Optimising it is a future step.

### Conflict-free with Step 36

This touches `error.rs`, `wire/http/mod.rs`, `wire/http/messages.rs` (test), and
`timeline/pipeline.rs` — all disjoint from the unmerged Step 36 (which touches
`asset_send.rs` / `asset_crypto.rs` / `ffi/timeline.rs` / `timeline/mod.rs`).
`encrypt_and_upload`'s signature is unchanged (the `room_id` is derived from
`group_id` internally), so no caller file — including Step 36's `asset_send.rs`
— is modified. Step 36's `send_file` inherits the self-heal automatically once
both merge, in any order.

### TDD

`wire/http` :
- `reject_from_response` maps `409 mls-stale-message` → `MlsStaleMessage`
- a `409` with another label still maps to `BackendRejected`

`timeline/pipeline.rs` (where a store-attached client harness allows):
- a send that gets `409 mls-stale-message` then `201` resyncs once and succeeds
- a second consecutive stale propagates (no infinite retry)
