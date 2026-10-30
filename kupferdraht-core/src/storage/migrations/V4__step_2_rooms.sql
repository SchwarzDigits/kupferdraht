-- Step 2 cycle 2d — rooms + room_members tables.
--
-- One row per conversation (proteus-only rooms are filtered at the
-- `wire::dto` → `Room` boundary and never reach this table). Members
-- land in the per-room `room_members` table, keyed by the composite
-- (room_id, user_id).
--
-- `name_ct` and `display_name_ct` follow the `_ct / _nonce / _tag`
-- convention from the first migration — plaintext bytes today,
-- AES-GCM ciphertext once Step 7's security baseline migration
-- rolls through (ADR-0003, docs/secure-storage.md).
--
-- `group_id` is nullable because Step 2 doesn't propagate it from
-- the Wire DTO to `Room` yet; Step 3 (Room join) adds the wiring
-- without a schema change.

CREATE TABLE rooms (
    room_id             TEXT    PRIMARY KEY NOT NULL,   -- "uuid@domain"
    group_id            TEXT,                           -- base64 MLS GroupId
    name_ct             BLOB    NOT NULL,
    name_nonce          BLOB,
    name_tag            BLOB,
    room_type           TEXT    NOT NULL,               -- "direct_message"|"group"|"channel"|"self_room"
    epoch               INTEGER NOT NULL,
    counterpart_user_id TEXT,                           -- qualified; NULL for non-DM
    last_synced_at_ms   INTEGER NOT NULL
);

CREATE TABLE room_members (
    room_id            TEXT    NOT NULL,
    user_id            TEXT    NOT NULL,                -- qualified
    is_self            INTEGER NOT NULL,                -- 0/1
    display_name_ct    BLOB,
    display_name_nonce BLOB,
    display_name_tag   BLOB,
    handle             TEXT,
    PRIMARY KEY (room_id, user_id),
    FOREIGN KEY (room_id) REFERENCES rooms (room_id) ON DELETE CASCADE
);

CREATE INDEX idx_room_members_room_id ON room_members (room_id);
