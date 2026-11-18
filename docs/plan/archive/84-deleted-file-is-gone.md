# Step 84 — A deleted file is gone: its key and its cached ciphertext go with the message

**Status: MERGED 2026-11-18** — `d271c0b` on `step/84-deleted-file-is-gone`, merged to
`main` as PR #101 (`9725a58`); locally fmt, 0 build warnings, clippy native and wasm
(both wasm lines), 1284 tests in 70 suites, the browser suite (18 suites, 89 tests);
CI green on the first run. Smoke 2026-11-17 on the local `wire-server-rs` stack: 18 ok
(see Smoke below), the stack's own 24 ok. Commissioned
2026-09-10, rewritten 2026-11-17 (Volker). First commissioned
as "asset deletion API + cache orphan cleanup" (open.md gap-B.3, gap-B.5, def-D.2): a
public `delete_asset`, a cache sweep, `cache list | size | clear` and an LRU size cap.
Rewritten after a look at the code: the case that matters was not in it — a file
deleted for everyone stays on the device, key included — while the public delete API
and the size cap are not needed, the cap also against the rule that the embedder
decides when to clean up. Step 80 (inbound-id path hardening), which the sweep had to
wait for, is merged.

## Problem

**A file deleted for everyone stays complete on the device.** A delete for everyone,
sent (`Timeline::redact`, `timeline/delete.rs`) or received (`sync/dispatch/delete.rs`),
ends in `apply_message_delete`, which empties the `timeline_events` row's body so that
"the row no longer carries the original message content at rest". The message's
`attachments` row is not touched: the raw AES key (`otr_key`), the asset token, the
locator and `cache_path` stay, and so does the ciphertext in
`<account>/assets/<message_id>.enc`. `Timeline::asset(message_id)` reads that row
without asking whether the message is deleted (`timeline/asset_receive.rs`), so a
deleted file can still be downloaded and opened. In the browser there is no cache file,
but the row with its key stays the same way. A deleted text is gone; a deleted file is
not.

**Cache files without a row stay forever.** `delete_attachment` exists in the store
but nothing calls it; an `.enc` file whose row is gone, or a staging file of a send
that was interrupted (`<account>/assets/staging/`), is never removed. Today that is a
smaller matter than the deleted file above, since rows are not removed anywhere else.

## Scope

1. **A delete for everyone removes the file's local traces.** In the transaction that
   tombstones the message, its `attachments` row goes; after the commit its cache file
   is removed. A failed removal is logged (`warn!`) and does not fail the delete — the
   sweep (2) catches it. Both directions, own and received deletes, native and
   browser. The redacted item keeps its tombstone; `Timeline::asset` of a deleted
   message answers `None`.
2. **One explicit sweep over the cache directory**: an `.enc` file that no row's
   `cache_path` names is removed. `assets/staging/` is skipped while a send may be
   running; a stale staging file is removed only where no send can be in flight (phase
   1). The embedder calls it; Kupferdraht runs no timer.
3. **Docs.** `THREAT_MODEL.md` (a deleted file at rest), `docs/library.md` and
   `docs/bindings.md` (`redact`, `asset`, the sweep), `docs/ARCHITECTURE.md` §7 if the
   store's contract changes, `CHANGELOG.md` under Security.

### Phase 1 questions (answered below)

- **The sweep's surface.** A call of its own on `Client`, or `cache size` / `cache
  clear` (the size as a reading the embedder asks for, clear as the sweep), with CLI
  verbs; or no public sweep at all, the deletes alone keeping the cache clean.
  Recommendation: one call, no size reading until an embedder asks.
- **The blob at cargohold.** Whether Wire's clients delete an asset at cargohold when
  its message is deleted for everyone (a reading of Kalium and the webapp). If they do,
  an own delete does the same through the existing `HttpClient::delete_asset`, best
  effort; if they do not, the blob stays to cargohold's retention and the transport
  call stays unused. Either way no public `delete_asset`.
- **Stale staging files**: removed at the sweep only when no send is in flight, or at
  the client's start before any send.

## Phase 1 (2026-11-17)

Read against the code; three gaps beyond the plan, and Volker's decisions:

- **A handle from before the delete.** An `AssetSummary` carries the key and the
  locator in memory; after the delete its `download` / `save_to` fetched the file
  again from the server, cached it anew and returned the plaintext. Removing row
  and file is not enough: a download now checks the row first, natively and in the
  browser, and answers `InvalidInput` (decided: no new error variant).
- **A failed upload.** The staged ciphertext was moved into the cache only on
  success; every failed `send_file` left a full encrypted copy in
  `assets/staging/`. The send now removes its staged copy however it ends.
- **A cleared history.** `clear_messaging_state` (a device that replaced a vanished
  one, at login) deletes every attachment row and no file.
- **Earlier deletes.** A file deleted before this step keeps its row and file; the
  migration `V36__step_84_forget_deleted_files.sql` drops those rows (approved with
  the red tests).
- **The blob at cargohold — decided (a).** Kalium deletes an own upload at the
  server after the delete went out, best effort; the webapp does not
  (`kalium-readings.md` #29, `asset-protocol.md` Q9). An own delete for everyone
  does as Kalium does; a received delete leaves the upload to its sender.
- **The sweep — decided (1), no public call.** A client's start
  (`register_device`, `resume`) empties `assets/staging/` — at a start no send can
  be running — and removes every `.enc` file no attachment row names. The
  remaining orphans (a crash between commit and removal, a failed removal, a
  download racing a delete, a cleared history) are Kupferdraht's own state; the
  app knows nothing about them Kupferdraht does not. No new public symbol, no new
  CLI verb.

## Smoke

2026-11-17, local `wire-server-rs` stack, throwaway team: 18 ok, 0 failed — A sends a
file, B downloads it, both hold a cached copy; A deletes it for everyone, nginz logs
`DELETE /v14/assets/…` → `200` (observations §106), A's copy is gone and its
`download-asset` refused; B syncs, its copy is gone, `download-asset` refused, nothing
saved, the timeline shows the deletion. The stack's `kupferdraht-smoke.sh --throwaway`:
24 ok, 0 failed.

## Retrospective

- **What the plan missed** — a handle the app already holds, the failed upload's staged
  copy, the cleared history, the files of earlier deletes — came out of reading the
  code in phase 1, not out of the tests the plan sketched (LEARNINGS 5.101).
- **The start's sweep assumes no send is running.** It holds through the manager, which
  keeps one live `Client` per account and hands it back instead of resuming again
  (Step 39), and across processes, which cannot open the same account. A second
  `ClientBuilder::resume` of a live account in the same process bypasses that; it could
  remove a staged copy mid-send, and the send would fail and could be resent.
- **Not probed:** a download after the server's delete (presumably `404`); whether a
  forward by another client references the same upload, which the server's delete
  would then take along (Kalium deletes as well).
- **Left as it was:** `Store::delete_attachment` stays unused; the delete removes the
  row inside its own transaction.

## Out of scope

- A public API to delete an asset at the server apart from its message.
- A size cap or LRU eviction of the cache: Kupferdraht offers readings, the embedder
  decides when to clean up.
- Copies the embedder wrote with `save_to`: outside Kupferdraht's reach.
- Blobs a cancelled or failed send left at cargohold.
- Previews of assets (`docs/non-goals.md` NG-2).

## TDD sketch

- Own `redact` of a file message: the `attachments` row is gone, the cache file is
  gone, `Timeline::asset` answers `None`; the timeline item is `Redacted`.
- A received delete of a downloaded file: the same; of a file never downloaded: the row
  is gone and nothing fails for the missing file.
- A cache file that cannot be removed: the delete still succeeds, the sweep removes it
  later.
- The sweep: a seeded orphan `.enc` is removed, a file a row names is kept, a file in
  `staging/` is kept.
- Browser path: the row of a deleted file is gone (the store tests run natively).
- Smoke: `send-file`, `redact` of it, `download-asset` refused on the sender and the
  receiver.
