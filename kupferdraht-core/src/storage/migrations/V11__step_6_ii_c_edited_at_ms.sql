-- Step 6-II-c — `edited_at_ms` column on `timeline_events`.
--
-- Set when the row's `body` was replaced by an inbound or
-- outbound `MessageEdit`. Nullable: most rows are not edits.
--
-- The model is **mutate the parent row in place** — Wire's edits
-- replace the body of the original message_id rather than
-- inserting a new row with a back-reference. This matches the
-- mental model of Wire web / iOS / android and keeps `Room::messages`
-- output stable (the timeline doesn't grow when an edit lands).
--
-- Idempotence guard for redelivery: the receive path applies an
-- edit only when the inbound `edited_at_ms` is strictly newer than
-- the persisted one (or when the row is currently unedited). This
-- prevents a redelivered older edit from rolling back a newer one.

ALTER TABLE timeline_events ADD COLUMN edited_at_ms INTEGER;
