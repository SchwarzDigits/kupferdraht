# Step 35 — Asset kind on the timeline read path

> **Status:** MERGED to `main` — `81aec4a`, PR #29 (squash-merge, no
> separate merge commit), 2026-06-18. Scope: fixes asset messages reading
> back as `Text` from `Client::messages`, surfaced by the natrium-ion
> file-message analysis. No dated live-smoke record (local read-path fix;
> its own TDD section lists store/unit-level tests only, no network path
> to verify).
>
> **Sub-steps:** none.
>
> **Historical names:**
> - "Step 35" — current.
>
> **Branch / commit prefix:** `step/35-*`, `fix(step-35): …`.

---

## Step 35 — Asset kind on the timeline read path

Reconstruct `TimelineItemKind::Asset` when reading the timeline from the store,
so `Client::messages` returns assets as assets — not as `Text`.

### The bug

`TimelineItem::from_record` (`timeline/items.rs`) maps a stored row's `kind`
string to a `TimelineItemKind`, but only handles `text` / `knock` / `location`
/ redacted — an `asset` row falls into the forwards-compat `else` branch and
becomes `TimelineItemKind::Text { body }`. Asset metadata (mime, filename,
size, asset locator) lives in the separate `attachments` table, which the pure
`from_record` cannot reach.

The **live** inbound path already emits the right thing
(`sync/dispatch/asset.rs` builds `TimelineItemKind::Asset` for the
`TimelineDiff`). But the **read** path `Client::messages`
(`client/timeline_read.rs`) hydrates reaction / receipt / mention overlays and
never loads attachments — so every re-read (app restart, conversation reopen,
or a poll-driven consumer like natrium that re-reads `messages()` each tick)
surfaces assets as `Text`. Consumers then render a file as a plain text bubble.

### Fix

`Client::messages` additionally loads the room's attachments
(`Store::list_attachments_for_room`, already present) and, after applying the
other overlays, replaces the kind of each matching, non-redacted item with
`TimelineItemKind::Asset(AssetSummary::from_attachment(...))` (the converter
already exists, used by the live path and `Client::asset`). Reaction / receipt
overlays still apply (a file can carry reactions); a redacted asset row stays
`Redacted`.

This mirrors the existing overlay pattern: one extra room-scoped query, applied
per item in the same loop.

### Scope note

Only the list read (`Client::messages` / `Timeline::items`) is fixed here. The
point lookup `Client::find_message_by_id` (Step 34) still uses the pure
`from_record`, so a reply *quoting* a file resolves to `Text` when the parent
is fetched via that path. In practice the quoting consumer resolves the parent
from the already-loaded `messages()` window first (now correct), so this is a
narrow edge; reconstructing assets in `find_message_by_id` can follow if needed.

### Public API

No signature change — `Client::messages` already returns `Vec<TimelineItem>`
whose `kind` can be `Asset`. The FFI `TimelineItemKind::Asset { asset:
AssetMeta }` is unchanged; only the *runtime value* for stored asset rows
changes (Text → Asset). Bindings are byte-identical.

### TDD

`client/timeline_read.rs` (or `timeline/items.rs` read tests):
- a stored `kind = "asset"` row with an attachment reads back as
  `TimelineItemKind::Asset` carrying the attachment's mime / filename / size
- a `text` row with no attachment is unaffected (still `Text`)
- a redacted asset row stays `Redacted`
- reactions still attach to an asset item
