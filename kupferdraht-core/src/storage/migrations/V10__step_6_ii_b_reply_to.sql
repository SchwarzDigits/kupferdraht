-- Step 6-II-b — `reply_to_id` column on `timeline_events`.
--
-- Stores the parent `message_id` for items that carry a
-- `Text.quote.quoted_message_id` (Wire's quote / reply hop).
-- Nullable: most items are not replies, and inbound replies
-- whose parent isn't (yet) in the local store still persist —
-- the column is a soft reference, not a foreign key.
--
-- Why no FK: we deliberately avoid coupling timeline rows to
-- each other in the schema. A reply may arrive before its parent
-- (Wire redelivery, paginated history backfill in a future Step)
-- and we want both rows to coexist. UI rendering looks up the
-- parent row by id at render time; if the parent is missing we
-- show a "(quoted message unavailable)" placeholder.

ALTER TABLE timeline_events ADD COLUMN reply_to_id TEXT;
