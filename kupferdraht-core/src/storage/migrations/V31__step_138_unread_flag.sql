-- Step 138: the hand-set unread flag beside the device-local read marker,
-- and a marker row that may have no position (a room flagged before it
-- was ever read). SQLite cannot drop NOT NULL in place, so the table is
-- rebuilt; the rows a V30 database holds keep their position, flag clear.
-- `last_read_at_ms` NULL means "never read" (everything inbound counts,
-- as a missing row did before); `marked_unread` 1 means flagged by hand.
CREATE TABLE room_read_markers_v31 (
    room_id          TEXT    PRIMARY KEY NOT NULL,  -- "uuid@domain"
    last_read_at_ms  INTEGER,                       -- NULL: never read
    marked_unread    INTEGER NOT NULL DEFAULT 0     -- 1: flagged by hand
);
INSERT INTO room_read_markers_v31 (room_id, last_read_at_ms, marked_unread)
    SELECT room_id, last_read_at_ms, 0 FROM room_read_markers;
DROP TABLE room_read_markers;
ALTER TABLE room_read_markers_v31 RENAME TO room_read_markers;
