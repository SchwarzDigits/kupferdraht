# Step 84 — A deleted file is gone: its key and its cached ciphertext go with the message

**Status: commissioned 2026-09-10, rewritten 2026-11-17** (Volker). First commissioned
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

### Phase 1

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
