-- Step 6-II-a — drop timeline_events.room_id FK CASCADE.
--
-- The V6 schema declared
--
--   FOREIGN KEY (room_id) REFERENCES rooms (room_id) ON DELETE CASCADE
--
-- which interacted badly with `Client::sync_metadata`'s
-- `replace_rooms` implementation: that path DELETEs every row from
-- `rooms` and re-INSERTs the fresh snapshot in one transaction, so
-- the CASCADE wiped every persisted `timeline_events` row on every
-- metadata refresh. Step 6-II-a's `Room::messages` reads
-- `timeline_events` directly, which made the bug visible — the
-- CLI's `timeline` showed only the *most recently drained* batch
-- of messages, not the full history.
--
-- The fix is to remove the CASCADE entirely. Going forward,
-- `timeline_events.room_id` is a soft reference. After
-- `Room::leave` (Step 6e-1) the row in `rooms` is removed by the
-- next `sync_metadata`; the corresponding `timeline_events` rows
-- stay around and remain readable via `timeline --id <room>`. That
-- is in fact the desired behaviour — leaving a room should not
-- silently destroy its history. A future "delete-history" CLI
-- can issue an explicit `DELETE FROM timeline_events WHERE
-- room_id = ?` if the user asks for it.
--
-- SQLite cannot ALTER an existing foreign key in place; the
-- canonical pattern is "create new table, copy data, drop old,
-- rename".

CREATE TABLE timeline_events_new (
    message_id        TEXT    NOT NULL,
    room_id           TEXT    NOT NULL,
    sender_user_id    TEXT    NOT NULL,
    sender_device_id  TEXT    NOT NULL,
    kind              TEXT    NOT NULL,
    body_ct           BLOB    NOT NULL,
    body_nonce        BLOB,
    body_tag          BLOB,
    sent_at_ms        INTEGER NOT NULL,
    delivery_status   TEXT    NOT NULL,
    PRIMARY KEY (message_id, room_id)
);

INSERT INTO timeline_events_new (
    message_id, room_id, sender_user_id, sender_device_id, kind,
    body_ct, body_nonce, body_tag, sent_at_ms, delivery_status
)
SELECT
    message_id, room_id, sender_user_id, sender_device_id, kind,
    body_ct, body_nonce, body_tag, sent_at_ms, delivery_status
FROM timeline_events;

DROP TABLE timeline_events;
ALTER TABLE timeline_events_new RENAME TO timeline_events;

CREATE INDEX idx_timeline_events_room_id ON timeline_events (room_id, sent_at_ms);
