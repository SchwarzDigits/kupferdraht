# Step 79 — Failed-send recovery API + explicit reconnect contract

**Status: implemented 2026-09-24, merged 2026-09-25** — `99082aa` (the
step: library + storage V28 + FFI + CLI + tests + docs) and `02c7df8`
(CI fix: purge per-host httpmock capture state at fixture setup —
LEARNINGS §5.13 recurrence) on `step/79-failed-send-recovery`, merged
to `main` by Volker, CI green; archived with this header. Commissioned
2026-09-10 (Kalium-audit follow-up — Volker: too critical to backlog).
**Scope shipped:** `Timeline::resend(message_id)` (same `message_id`,
the `SendHandle::unwedge` shape — text / reply / mentions, ephemeral
re-wrapped with its persisted TTL, knock, location, asset reference
without a second blob upload; task / case / marker snapshots refused,
open.md NG-17), `Timeline::resend_failed() -> ResendSummary`
(oldest-first, stops at the first hard error), `Room::failed_sends()`;
V28 `timeline_events.ephemeral_ttl_ms` + `Store::mark_message_resent`;
FFI mirrors + `ffi::ResendSummary` / `FailedResend`; CLI `resend` /
`failed-sends`; the reconnect contract written into `docs/library.md`
(mirrored in `docs/bindings.md`); the Step-89 task-backfill SELECT
fixed on the way (LEARNINGS §5.33). **Smoke 2026-09-24**
(schwarz-dev-wire, own homes 79-1/79-2): three sends failed behind a
dead HTTPS proxy via the library path, `failed-sends` listed them,
`resend --message` + `resend --all` re-drove them under their ids, the
receiver drained exactly three events, the ephemeral with its timer
(observations §84). Follow-up (open.md Proposed steps): kupferdraht-tui
Ctrl-R onto the SDK API.

## Problem (verified 2026-09-10)

The Kalium JVM audit found a resend scheduler that logs into the
void — offline-failed messages are never retried and nothing tells
the consumer. Kupferdraht fails *honestly* but leaves the recovery
entirely to the consumer:

- A failed send persists its row with `DeliveryStatus::Failed` and
  keeps the plaintext body — but the core never retries, and there
  is no API to retry: kupferdraht-tui had to build Ctrl-R by re-reading
  the row and re-driving the original send path (its Step 11,
  `PendingKind` Text/Reply/Edit/File), which every consumer would
  have to reimplement.
- The one existing self-heal is narrow: a stale-epoch rejection
  re-encrypts and resends ONCE inside the send call
  (`timeline/pipeline.rs:58`) — that covers epoch races, not
  network failures.
- Reconnect is likewise consumer-owned: `sync`/`sync_stream` return
  on hard error by design (docs/library.md §sync), the TUI restarts
  with 1s→60s backoff. The contract is real but stated nowhere as
  a contract; there is no network observer in core (deliberate —
  OS network state is platform territory).

## Scope

1. **`resend` API on the failed row** — orientation checks the
   matrix-rust-sdk shape first (its send-queue exposes per-message
   retry; ADR-0009 naming). Sketch: `Timeline::resend(message_id)`
   re-drives the original send from the persisted row (text incl.
   reply/mentions, task, case, marker, asset re-upload?) +
   `Room`-level "resend all failed, oldest first". Asset resends
   may need the cached plaintext — scope decision in phase 1
   (possibly text-family first, assets follow-up).
2. **No background scheduler in core** (deliberate — the core has
   no lifecycle to hang it on; consumers own their loop). Instead:
   the failed rows are enumerable (`Room::failed_sends()` or a
   timeline filter) so a consumer can wire "sync went Live →
   resend" in three lines.
3. **Write the reconnect contract down**: library.md sync section
   gets the explicit paragraph ("the stream dying on hard error is
   the contract; reconnect/backoff and resend-on-recovery are the
   consumer's loop — no network observer in core"), mirrored short
   in bindings.md for the FFI consumers.
4. FFI + CLI (`resend --id <room> --message <id>` or similar) in
   the same step; kupferdraht-tui's Ctrl-R migrates to the SDK API as
   the consumer follow-up.

## TDD sketch

- Failed text/reply send → `resend` → same `message_id`? (decide:
  new id, matrix-style, vs. reuse — orientation) → row flips to
  Sent, one upload hit.
- Resend of a non-failed row → `InvalidInput`; resend keeps
  mentions/reply metadata (pin via mock capture).
- Room-level resend sends oldest-first, stops surfacing on first
  hard error but leaves remaining rows Failed (re-runnable).

## Orientation (2026-09-24, execution)

Plan of 2026-09-10 re-verified against `main@b7b13e8` (Steps 77, 86–92
landed since): the stale-epoch self-heal now lives in
`timeline/pipeline.rs` `Client::encrypt_and_upload` (not line 58 — the
file grew), every send path still persists `Failed` through
`DeliveryStatus::for_send`, and Step 89 added the ephemeral send
family whose failed rows keep the body but no timer.

### matrix-rust-sdk shape → Kupferdraht derivation

matrix-rust-sdk 0.17 (`matrix_sdk::send_queue`, read via its published
docs — the submodule is not checked out in this worktree): sends go
through a per-room `RoomSendQueue`. A *recoverable* failure (transient
network, 5xx) is retried by the queue itself; an *unrecoverable* one
marks the local echo **wedged** (`EventSendState::SendingFailed {
is_recoverable: false }`) and — since 0.17 — blocks every later request
of that room (strict order) until the consumer calls
`SendHandle::unwedge()` (same transaction id) or `abort()`.
`SendQueue::respawn_tasks_for_rooms_with_unsent_requests()` restarts
the per-room tasks after a reconnect.

Kupferdraht has no queue and, by plan, no scheduler: the persisted `Failed`
row *is* the wedged local echo and its `message_id` *is* the
transaction id. Derived surface (ADR-0009 naming):

| Kupferdraht | matrix-rust-sdk | Rationale |
|---|---|---|
| `Timeline::resend(message_id) -> MessageHandle` | `SendHandle::unwedge()` | explicit per-message retry; the handle is the row id because the row outlives the process |
| `Room::failed_sends() -> Vec<TimelineItem>` | the `SendingFailed` echoes a `RoomSendQueue::subscribe()` yields | enumerable, oldest-first, so a consumer wires "sync went Live → resend" in three lines |
| `Timeline::resend_failed() -> ResendSummary` | `SendQueue::respawn_tasks_for_rooms_with_unsent_requests()` scoped to one room | oldest-first; stops at the first hard error (matrix's strict-order rule — a stuck message must not be overtaken); rows behind the stop stay `Failed`, the pass is re-runnable |

**Same `message_id` on the wire.** matrix keeps the transaction id
across `unwedge`; for Kupferdraht it is also the safe choice: if the first
attempt did reach the backend behind a 5xx proxy, receivers dedupe on
`message_id` instead of showing the message twice. `sent_at_ms` is
re-stamped to the resend instant — the row moves to where recipients
will see it (ordering is by `sent_at_ms`); a resent ephemeral row gets
`expires_at_ms = new sent_at_ms + ttl`.

### Resend family (V1), verified against today's send paths

- **In:** text (plain / reply / mentions — mentions come from the
  `mentions` table, the quote is re-derived from the persisted parent
  like `send_reply`), **ephemeral text** (Step 89 kept the failed body
  *for a resend* but persisted no TTL — a resend could only have sent
  the OTP as a permanent text; V28 adds `timeline_events.ephemeral_ttl_ms`
  on own ephemeral sends so the resend re-wraps with the original TTL,
  stamps the expiry and arms the timer), knock, location (self-contained
  bodies), and **asset**: the plan's "re-upload needs the cached
  plaintext" does not apply — `finish_asset_send` persists a row only
  after `POST /v14/assets` succeeded, and the `attachments` row carries
  the complete reference (raw key, sha256, asset id/token/domain), so
  the resend rebuilds the `Asset` message from it without a second blob
  upload (closes kupferdraht-tui's `PendingKind::File`).
- **Out (refused with `InvalidInput`, listed as `skipped` by
  `resend_failed`, still visible in `failed_sends()`):** task / case /
  marker rows are full-state LWW snapshots (ADR-0014/0018/0020);
  replaying a stale snapshot can regress every receiver's fold. The
  recovery that exists is re-sending the *current* state
  (`update_task` / `set_case` / `set_task_list`).
- **Not a `Failed` row at all:** edit, redact, reaction, receipts,
  typing propagate the error *before* mutating locally
  (`edit_message` uploads with `?` first) — kupferdraht-tui's
  `PendingKind::Edit` stays a consumer-side retry.

### Storage

V28 `ALTER TABLE timeline_events ADD COLUMN ephemeral_ttl_ms INTEGER`
(own ephemeral sends only; inbound rows keep the derived
`expires_at_ms`). `Store::mark_message_resent(room_id, message_id,
sent_at_ms, expires_at_ms) -> bool` — one UPDATE guarded by
`delivery_status = 'failed'`; the default body for custom stores
returns `Ok(false)` and the core turns that into `KupferdrahtError::Storage`
(never a silent success).

### Smoke mechanics

Every room command of the CLI pre-syncs, so a cut network fails the
sync, not the send. Procedure: (1) CLI `sync` refreshes the token;
(2) a throwaway program (scratchpad, never committed) resumes the same
home through `Kupferdraht::resume` **with `HTTPS_PROXY=http://127.0.0.1:1`**
— the library path has no pre-sync, the token is fresh, MLS encryption
is local, the POST dies at the dead proxy → `Failed` row; (3) CLI
`failed-sends` lists it, `resend` re-drives it; (4) the peer's
`timeline` shows the message exactly once.
