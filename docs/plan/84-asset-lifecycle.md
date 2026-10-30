# Step 84 — Asset deletion API + cache orphan cleanup

**Status: commissioned 2026-09-10** (transferred from open.md
gap-B.3 + gap-B.5; def-D.2 names the missing mini-ADR this step
writes first).

## Problem

1. **gap-B.3**: `HttpClient::delete_asset` exists (transport layer)
   but nothing exposes it — no `Timeline::delete_asset`, no
   `kupferdraht-cli delete-asset`. A sender cannot remove an uploaded
   blob from the server.
2. **gap-B.5**: `delete_attachment` drops the DB row but leaves the
   `.enc` cache file — the cache accumulates orphans forever.

## Scope

1. Phase-1: run `new-adr` for the small asset-deletion decision
   (def-D.2): who may delete (sender-only per Wire semantics?),
   what happens to the timeline row (redaction interplay with
   `MessageDelete`), remote-delete on redaction yes/no.
2. `Timeline::delete_asset(message_id)` per the ADR + FFI mirror +
   CLI `delete-asset`.
3. Orphan cleanup: deleting the attachment row removes its `.enc`
   cache file (both the delete API and the existing redaction
   path); a `Client::asset_cache_sweep()` (or equivalent) removes
   `.enc` files with no matching row — decided in the ADR whether
   it runs implicitly or stays an explicit call.
4. Orphan **blobs** on the server (added after the Step 97a review,
   2026-09-29): a cancelled file send whose blob upload the server had
   already stored leaves a blob without a row, and a `Failed` file row
   deleted instead of resent leaves its blob — decide whether deleting
   the row deletes the blob remotely too.
5. Note: the cache path/id handling must land AFTER Step 80
   (inbound-id path hardening) — the sweep walks the cache dir and
   must not be confused by hostile names.

## Out of scope

- gap-B.4 (LRU eviction / size cap) — needs a sizing policy
  decision; stays in open.md's backlog.

## TDD sketch

- delete_asset: server call hit, row handling per ADR, cache file
  gone; non-own asset → error per ADR.
- Orphan sweep: seeded orphan `.enc` removed, live-referenced file
  kept; redaction path removes the cache file.
