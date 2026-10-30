# Step 97 — Durable send and receive: persist before upload, no push gap, atomic MLS state

**Status: MERGED 2026-09-29** — commissioned 2026-09-28 (Markus —
blockers B6–B8 of the production-readiness review, `open.md` §71; 97e
added for B9, found in 97c). One branch + PR per sub-step: **97a**
`1a500fa` + `ea02553`, PR #61 (`78ad6dd`); **97b** `b49cef5` (ADR-0030)
+ `bc051c4`, PR #63 (`956092a`); **97c** `412ef3d`, PR #64 (`38dcb2b`)
— all three merged 2026-09-28; **97d** `d8d52ad` (ADR-0031) + `6244eb6`
+ `1631205`, PR #65 (`0b6b366`); **97e** `0dcc8bc` (ADR-0032) +
`0f02612` + `15faadd`, PR #66 (`53906db`) — merged 2026-09-29. CI green
on each, except #65's browser job: the flaky Step-94 takeover test that
Step 100 fixed. Smoke: every sub-step live against schwarz-dev-wire on
2026-09-28 (release CLI). Scope shipped: sends persisted as `Sending`
before the upload; the legacy push socket is a doorbell for the REST
drain; one transaction per MLS operation; one drain per account,
deferred around own commits; an inbound notification's MLS state
committed after its data. Retrospectives: `docs/LEARNINGS.md` §5.37
(97b), §5.38 (97c), §5.39 (97d), §5.40 (97e). Residuals: `open.md` §71
(the wasm `send_bytes` path emits no `Append` for its own row; custom
`Store`s stay non-atomic). Archived with this header.

## Problem (verified in code 2026-09-27/28)

1. **B7 — sends are persisted only after the upload.** Every send path
   that creates its own row — text (incl. reply, mentions), ephemeral,
   knock, location, file, task, case, task-list marker — awaits
   `encrypt_and_upload` (`timeline/pipeline.rs`) before the row is
   written (`timeline/text.rs` `Client::send_text`, and the siblings).
   A call cancelled during the upload (a Kotlin coroutine cancel drops
   the Rust future; a crash) leaves no row and nothing to resend. A
   persist failure after a successful upload is returned as an error —
   the user retries under a new message id and the message goes out
   twice. Edits, deletes, reactions and receipts have no row of their
   own; their retry is idempotent or chains an edit — out of scope.
2. **B6 — the legacy `/await` push loses events.** `sync/push/legacy.rs`
   drains `GET /notifications?since=` first and connects afterwards;
   every durable socket frame advances the persisted cursor
   (`sync/service.rs::dispatch_notification`). An event stored between
   the end of the drain and the socket registration is never pushed,
   and the next durable frame moves the cursor past it — no later drain
   returns it. The same happens to any frame cannon fails to push while
   connected (best effort). ADR-0019 §2–3 ("drain first closes any
   gap", cursor per frame) mandate this mechanism. Current wire-server
   ships `/events` hard-disabled, so `/await` is the push path in
   practice. `/events` itself is not affected (server queue + ack).
3. **B8 — `mls.db` is neither atomic nor serialised.** openmls writes
   through `openmls_sqlite_storage` 0.3.0 one autocommit statement per
   record; a commit merge is about 14 writes (the pending commit is gone
   after the first). A crash in between leaves tree, context and
   secrets at different epochs; a Welcome whose key package was deleted
   but whose group was not stored can never be retried. openmls expects
   the application to wrap operations in a transaction; Kupferdraht does not
   (`encryption/provider.rs` owns the `Connection`). Across processes
   or `Client`s, MLS mutations are not serialised at all (the Step-85
   lock covers key creation and token refresh only); multi-step flows
   (stage commit → upload → merge / clear / discard) span network I/O.

## Decisions (Markus, 2026-09-28)

- **D1 (97a) — persist first, with a `Sending` state.** A row-producing
  send writes its row (mentions, attachment) as
  `DeliveryStatus::Sending` **before** the upload and emits the Append;
  after the upload a guarded update moves it to `Submitted` or `Failed`
  (never downgrading a row the echo already marked `Received`). A new
  enum variant — `BREAKING:` for exhaustive `switch` / `when`.
- **D2 (97a) — an abandoned `Sending` row reads as `Failed`.** The
  `Client` tracks its in-flight sends; a `Sending` row not in flight on
  this `Client` (the future was dropped, or the process restarted — and,
  corrected after review in Step 107, a live send of another `Client`
  on the same store) is
  reported as `Failed` and is resendable through Step 79's resend under
  the same message id — no write at startup, nothing flipped behind
  another process's back.
- **D3 (97a) — an upload that succeeded is `Ok`.** If only the status
  update after a successful upload fails, the send returns `Ok` and
  warns; the row stays `Sending` → reads as `Failed` → a resend goes out
  under the same id, and receivers dedupe on it. Never an error that
  invites a second send under a new id.
- **D4 (97b) — the socket is a doorbell.** A durable `/await` frame no
  longer advances the cursor or dispatches by itself: it triggers a
  REST drain from the cursor (coalesced while frames keep arriving);
  only the drain advances the cursor. Transient frames (typing) still
  dispatch directly. Closes both the connect window and frames lost
  while connected. Costs one REST round trip per burst. New ADR
  superseding ADR-0019 §2–3, written before 97b's red tests.
- **D5 (97c) — one transaction per MLS operation.** Every `MlsClient`
  operation runs inside one `BEGIN IMMEDIATE … COMMIT` on the
  `mls.db` connection (rollback on error or unwind), through a
  borrowed-connection storage provider. Atomic merges, Welcomes and
  external joins; single operations serialised across processes and
  `Client`s via SQLite's write lock + `busy_timeout`; about one fsync
  per operation instead of ~15.
- **D6 (97d, own design round) — serialise multi-step MLS flows.** A
  dedicated lock around stage → upload → merge / clear / discard and
  around inbound dispatch batches, with a bounded wait — a departure
  from ADR-0024's fail-fast, so a new ADR first. **Replaced
  2026-09-28 by ADR-0031** (Markus): a lock held across the upload
  would make every other MLS operation and drain wait a network round
  trip and would not stop two drains from processing the same
  notification. Instead: one cursor-advancing dispatch per account at a
  time (`sync.lock`, never across the network), and a next-epoch
  message for a group with an own pending commit is deferred, not
  healed; old-epoch messages are dropped.

## Sub-steps

### 97a — Persist before upload (B7)

- `DeliveryStatus::Sending` (`"sending"`; the column has no CHECK — no
  migration). Old binaries read it as `Submitted` (downgrade only).
- Row-producing sends: insert `Sending` + emit Append → upload → guarded
  outcome update + `UpdateAt` diff. Ephemeral: `expires_at` and the
  timer only on success (as today). File: the row + attachment after the
  blob upload, before the MLS upload (a cancelled blob upload leaves no
  row, as today). Task: the fold-cache upsert keeps its place.
- In-flight tracking on `Client` with an RAII guard (no async work in
  `Drop`); reads map `Sending` ∉ in-flight → `Failed`; `failed_sends`,
  `resend`, `resend_failed` and `mark_message_resent` accept it;
  `mark_delivery_received` upgrades `Sending` and `Submitted`.
- FFI (`DeliveryStatus` is a UniFFI enum), wasm projection, CLI
  (`timeline` shows the state), docs.

TDD (sketch): a send whose upload never answers is cancelled
(`timeout`) → the row exists, reads as `Failed`, `failed_sends` lists
it, a resend delivers the same message id; the Append is emitted before
the upload; a store that fails the outcome update after a successful
upload → `Ok`, and a resend reuses the id; an echo that arrives before
the outcome update is not downgraded; a restart → the abandoned row is
`Failed`.

**As built (2026-09-28).**
- `timeline/send_state.rs`: the `Client`'s in-flight set with an RAII
  guard; `settled_status` / `settle_items` for reads; `is_resendable`;
  `finish_send` (never an error after the upload); `send_content_row`
  (knock, location, case, marker share it). `Store::mark_send_outcome`
  (defaulted, guarded `WHERE delivery_status = 'sending'`);
  `mark_delivery_received` and `mark_message_resent` accept `sending`.
- Ephemeral: the expiry is `sent_at + ttl` as before (Step 89's
  contract), set only on delivery — so it now arrives with the settling
  `UpdateAt`, not with the `Append` (`step_89` adjusted).
- Tests: `tests/step_97a_persist_before_upload.rs` (5 red → green) with
  `tests/common/faulty_store.rs` — a `Store` that forwards all methods to
  the real one and fails chosen ones on demand (generated from the trait;
  reusable). Storage unit test: the outcome update never downgrades an
  echo. Gate 2026-09-28: 948 passed.
- Found on the way (not fixed, `open.md` §71): the wasm `send_bytes`
  path emits no `Append`.
- Smoke 2026-09-28 (release CLI, schwarz-dev-wire): a normal send
  settles to `→` and user 2 receives it; `kupferdraht-cli send` SIGKILLed at
  0.36 s (after the row, during the upload) left a row that reads `✗`
  after the restart and is listed by `failed-sends` (before 97a: no row
  at all); `resend` delivered it under the same id — user 2 has it
  exactly once. Kotlin and Swift bindings generate with the new variant.

### 97b — The push socket is a doorbell (B6)

- ADR first (supersedes ADR-0019 §2–3); fix observations §68 and
  `docs/wire/api-notes.md`, which repeat "drain first closes any gap".
- Legacy cycle: connect → drain → frames: durable → coalesced drain,
  transient → dispatch. The drain after the connect is the doorbell
  for the backlog (no drain before the connect any more).

TDD (sketch, `tests/step_73_legacy_await_push.rs` harness): an event
stored while the socket connects is delivered after the next frame;
a frame whose event the drain already returned is not dispatched twice;
the cursor ends at the newest drained id; transient frames still
dispatch without a drain.

**As built (2026-09-28).**
- ADR-0030 (Accepted, Markus) supersedes ADR-0019 items 2–3; no
  periodic drain on the keepalive tick (a live socket needs none).
- `sync/push/legacy.rs`: connect → the doorbell loop. A per-connection
  `Doorbell` (drain owed, held transient frames, frames received); the
  drain is a boxed `Fuse` future selected alongside the socket read and
  the ping tick, so the socket is read while a drain runs and a drain is
  never cancelled — a socket that ends mid-drain lets it finish first.
  Transient frames read during a drain are held (cap 64, oldest
  dropped) and dispatched right after it, so they never interleave
  with the drain's dispatch cycle. The consumer ticks once per settled
  drain or transient frame; the reconnect backoff still resets only on
  frames the socket delivered.
- Tests: `tests/step_97b_push_doorbell.rs` (4 red → green, a transient
  pin, and a pin for the drain-failure reconnect — open.md gap-B.16's
  Step-73 residual). `step_73`'s cursor-per-frame and
  drain-before-streaming tests rewritten to the doorbell. Gate
  2026-09-28: 957 passed.
- Docs: observations §68 carries a correction note; api-notes,
  ARCHITECTURE, library, cli.md, the `SyncTransport` doc and the CLI
  banner describe the doorbell.
- Smoke 2026-09-28 (release CLI, schwarz-dev-wire, `sync-stream
  --transport websocket`): a message sent while user 1 was offline
  arrived through the connect drain; three back-to-back messages each
  rang a drain and arrived once; a REST typing from user 2 arrived as a
  transient frame; a message sent during the reconnect rang 34 ms after
  the connect and arrived once.

### 97c — Atomic MLS operations (B8, atomicity)

- `KupferdrahtProvider` owns the `Connection`; `with_tx` opens
  `BEGIN IMMEDIATE` and hands openmls a provider over
  `&Connection`; nested calls reuse the running transaction.
- Every mutating `MlsClient` method runs in one transaction; read-only
  multi-row loads in a deferred one.

TDD (sketch): an injected failure in the middle of a merge (a SQLite
trigger on `openmls_group_data`) leaves the group at the old epoch with
its pending commit, and a retry reaches the new epoch; the same for a
Welcome (the key package survives a failed absorb); a concurrent writer
holding the write lock makes an MLS operation wait, not interleave.

**As built (2026-09-28).**
- `encryption/provider.rs`: `KupferdrahtProvider` owns the `Connection`;
  `with_write_tx` (`BEGIN IMMEDIATE`) / `with_read_tx` (deferred) hand
  openmls a per-operation `TxProvider` whose storage is
  `SqliteStorageProvider<JsonCodec, &Connection>` (supported by
  `openmls_sqlite_storage` 0.3 as is — no fork). rusqlite's
  `Transaction::new_unchecked` rolls back on drop, so an error and an
  unwind both roll back; a call inside a running operation joins it (a
  write inside a read snapshot is asserted impossible).
- Every mutating `MlsClient` operation is wrapped (create, add, kick,
  merge, clear, self-update, external join, external removals,
  Welcome, encrypt, process, discard, key-package batch, signature-key
  store); reads go through `load_group_snapshot`. Unit-test fixtures
  that build raw openmls groups use a test-only `autocommit_provider`.
- Tests: `encryption/atomicity_tests.rs` (3 red → green) — a SQLite
  trigger (`RAISE(ABORT)`) injects the fault through a second
  connection; a `commit_hook` counts commits (rusqlite `hooks`, native
  dev-dependency only). The plan's "concurrent writer waits, does not
  interleave" test became "every operation commits exactly once": an
  interleave inside an openmls operation cannot be provoked
  deterministically without a hook in openmls, and one `BEGIN
  IMMEDIATE` transaction per operation rules it out. Before: key-package
  batch of 3 = 3 commits, create 11, merge 13, Welcome 12, discard 13.
- Two pre-existing over-long functions touched on the way were split
  (`absorb_welcome` → `parse_welcome`, `run_external_commit` →
  `external_join_config`). Gate 2026-09-28: 960 passed.
- Smoke 2026-09-28 (release CLI, schwarz-dev-wire, smoke homes whose
  `mls.db` predates 97c): a message in an existing room sent and
  received; a new room created, user 2 invited and joined via the
  Welcome, messages both ways, user 2 removed.

### 97d — Serialise the drain, defer around own commits (B8, ADR-0031)

- `sync.lock` (advisory file lock, bounded wait) + an async mutex per
  account guard every cursor-advancing dispatch; the page is fetched
  outside the lock and refetched when the cursor moved under it;
  `end_dispatch_cycle` (network) runs outside.
- A per-group async lock around stage → upload → merge/clear; the drain
  waits on it for a next-epoch message of that group; without an
  in-flight flow it stops before the message; an abandoned pending
  commit (past the bound) is cleared and healed as today.
- Old-epoch messages are dropped, not healed (part of H4).

TDD (sketch): two `Client`s on one account drain the same queue → each
notification is dispatched once; a next-epoch message arriving during
an own commit's upload is decrypted after the merge, no rejoin; a
redelivered old commit is skipped, no rejoin; a pending commit past the
bound is healed as today; the lock is not held while the page is
fetched.

**As built (2026-09-28).**
- `encryption/messages.rs`: `process_inbound_message` sorts by epoch
  before openmls — `StaleEpoch` (older), `AwaitsOwnCommit` (next epoch
  + own pending commit); nothing is written for either.
- `sync/drain.rs` (moved out of `service.rs`): fetch → `lock_drain` →
  cursor check (moved → refetch) → dispatch → release; a deferred
  notification stops the page; `deferral_step` (pure, unit-tested)
  decides wait for this process's flow / stop / abandon after 60 s.
  `client/drain_lock.rs`: in-process mutex + `sync.lock`, retried for
  5 s (`AccountLocked` after). The dispatch chain returns outcome enums
  (`NotificationOutcome`, `EventOutcome`); a deferred notification does
  not advance the cursor.
- `sync/follow_ups.rs`: healing (`resync_room`), 1:1 delivery receipts
  and config/property re-fetches are queued during dispatch and run in
  `end_dispatch_cycle`, after the lock (the config arms became sync).
- `client/commit_flow.rs` + `Client::stage_and_upload_commit`: invite,
  kick, room / DM establishment run stage → upload → merge/clear under
  the group's async lock; the removal flush takes it directly;
  `upload_and_merge_commit_bundle` asserts it is held.
- `/v14/events`: the frame dispatch takes the drain lock; a deferred
  frame stays unacked and the connection is cycled for its redelivery.
- Tests: `tests/step_97d_serialise_drain.rs` (4 red → green) with real
  MLS (`PeerGroup::new_mixed_plaintext`, `process_commit`); the second
  `Client` resumes on the same home (Step-85 precedent); a rejoin is
  observed as `GET …/groupinfo`. Unit tests for `deferral_step`. The
  95c hostile-property test caught a `debug_assert!` on a server-sent
  key in the new re-fetch follow-up — removed. Gate 2026-09-28: 967
  passed.
- Found by the smoke, fixed here: every writing transaction of
  `kupferdraht.db` started `DEFERRED`; a read-then-write one (the ephemeral
  sweep) failed `database is locked` at once, not after `busy_timeout`,
  when the other process committed while it waited. All twelve start
  `IMMEDIATE` now; regression test
  `a_read_then_write_transaction_waits_for_another_writer` (red with the
  smoke's exact error → green). Gate: 968 passed.
- Not changed (pre-existing length debt, open.md §70): `drive_connection`
  and `commit_pending_external_removal` grew by a few lines.
- Smoke 2026-09-28 (release CLI, schwarz-dev-wire): user 1 creates a
  room, invites user 2, sends three messages there and two into an
  existing room; user 2 runs two `sync` processes on one home at once —
  both exit 0, every message arrives once, one process refetched after
  the other moved the cursor, no heal. Then user 1 removes user 2 and
  writes again — same result. (A first run before the store fix failed
  one process with `database is locked`; another first run got
  `AccountLocked` from two simultaneous resume-time token refreshes —
  Step 85's documented fail-fast, not this step.)

### 97e — Persist before the MLS commit (B9, ADR-0032)

Found in 97c's retrospective (open.md §71 B9): decrypting commits the
ratchet advance to `mls.db` before the plaintext reaches `kupferdraht.db`; a
crash in between loses the message (its key is gone). Decided
2026-09-28 (Markus): at-least-once processing — per notification one
`mls.db` write window around decrypt + persist, MLS commit after the
persistence, cursor after the MLS commit; idempotent handlers; a replay
skips what MLS already processed; an async MLS write gate keeps every
other MLS write out of the window.

TDD (sketch): a `kupferdraht.db` failure while persisting an inbound message
rolls the MLS state back (the message decrypts on the next drain); a
replayed notification after the MLS commit is skipped without a sync
error and without a second callback; a send during a notification's
window waits and neither is lost; every inbound handler is idempotent
per message id.

**As built (2026-09-28).**
- `KupferdrahtProvider`: `begin_window` / `commit_window` / `rollback_window`
  (one `BEGIN IMMEDIATE` held across the notification);
  `with_inbound_tx` lets `process_inbound_message` and `absorb_welcome`
  join it; any other write while a window is open fails closed.
  `sync/window.rs::NotificationWindow` takes the MLS write gate, opens
  the window and rolls back on drop (error, deferral, unwind).
- `dispatch_notification`: window → events → commit → cursor. Event
  failures are split (`EventError`): a `kupferdraht.db` failure after the MLS
  step (`Persist`) aborts the notification and the sync call; anything
  else is skipped / healed as before.
- `process_inbound_message`: a consumed key (`SecretReuseError`) →
  `AlreadyDecrypted`, skipped quietly.
- `client/mls_gate.rs`: the MLS write gate, taken by every MLS write
  outside inbound dispatch — encrypt, commit staging / merge / clear,
  key packages, group create / join / discard / leave / resync, the
  abandoned-commit clear.
- Tests: `tests/step_97e_persist_before_mls_commit.rs` (4 red → green);
  `FaultyStore` can park one call (`pause_next` / `release`);
  `FaultyProvider` moved to `tests/common`. Gate 2026-09-28: 973 passed.
- Handler audit: text, knock, location and asset notify only on a fresh
  row; reactions replace the sender's set; edits ignore one already
  applied; deletes redact; receipts are `INSERT OR IGNORE`; tasks upsert.
  **Residual** (open.md §71): an event with two `kupferdraht.db` writes (text
  row + mentions, asset row + attachment) is not atomic in `kupferdraht.db` —
  a failure between them rolls back and replays, but the replay sees the
  row and skips the second write (and the callback).
- Pre-existing length debt grew by a few lines in
  `join_room_by_external_commit`, `create_room`, `establish_self_room`,
  `commit_pending_external_removal` (the gate).
- Smoke 2026-09-28 (release CLI, schwarz-dev-wire): user 2 sends 40
  messages; user 1's `sync` is SIGKILLed at 0.10–0.28 s in turn — one
  kill landed mid-drain after 14 messages, the next run received exactly
  the remaining 26; a final sync ends clean; every message exactly once
  in the timeline; a reply from user 1 reaches user 2. (Counting through
  `timeline` between kills is misleading — it drains first.)

## Out of scope

- Row-less sends (edit, delete, reaction, receipts) — their retry is
  idempotent or chains an edit.
- H4 (MLS healing too broad) and H5 (blocking work in async code) —
  they interact with 97c/97d and stay in `open.md` §71.
- gap-B.22 (no staged external join) and gap-B.27 (non-atomic `mls.db`
  migrations).
