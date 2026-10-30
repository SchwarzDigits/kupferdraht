-- Step 67: device-local read position per room. Separate table (not a
-- rooms column) because `rooms` is replace-managed by every metadata
-- sync. Everything with sent_at_ms <= last_read_at_ms counts as read;
-- a missing row means "never marked" → all inbound counts as unread.
-- Cross-device sync of this position rides the self-conversation in a
-- later step.
CREATE TABLE room_read_markers (
    room_id          TEXT    PRIMARY KEY NOT NULL,  -- "uuid@domain"
    last_read_at_ms  INTEGER NOT NULL
);
