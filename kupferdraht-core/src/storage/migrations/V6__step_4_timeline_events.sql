-- Step 4 cycle 4e — timeline_events table.
--
-- One row per outgoing or incoming text message. Step 4 only writes
-- outgoing rows (`kind='text'`, `delivery_status='submitted'`); Step
-- 5 extends with incoming rows and upgrades submitted →
-- acknowledged where applicable.
--
-- `body_ct` follows the `_ct / _nonce / _tag` convention from the
-- earlier migrations — plaintext today, AES-GCM ciphertext after
-- Step 7 (ADR-0003, docs/secure-storage.md). The plaintext stored
-- is the `Text.content` field of the `GenericMessage` Protobuf, not
-- the MLS ciphertext (which is a transient transport artifact).
--
-- The composite (`message_id`, `room_id`) is the natural key — Wire's
-- `GenericMessage.message_id` is a UUID generated client-side, so a
-- collision across rooms is astronomically unlikely but the
-- composite is cheap and disambiguates.

CREATE TABLE timeline_events (
    message_id        TEXT    NOT NULL,                -- GenericMessage.message_id (UUID v4)
    room_id           TEXT    NOT NULL,                -- "uuid@domain"
    sender_user_id    TEXT    NOT NULL,                -- qualified user id
    sender_device_id  TEXT    NOT NULL,                -- Wire client_id (hex)
    kind              TEXT    NOT NULL,                -- "text" today; "system" reserved
    body_ct           BLOB    NOT NULL,
    body_nonce        BLOB,
    body_tag          BLOB,
    sent_at_ms        INTEGER NOT NULL,
    delivery_status   TEXT    NOT NULL,                -- "submitted" | "acknowledged" | "failed"
    PRIMARY KEY (message_id, room_id),
    FOREIGN KEY (room_id) REFERENCES rooms (room_id) ON DELETE CASCADE
);

CREATE INDEX idx_timeline_events_room_id ON timeline_events (room_id, sent_at_ms);
