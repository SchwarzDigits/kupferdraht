-- Step 130a: idempotency keys. One row per key a keyed send was called
-- with, and the outcome the key answers: the handle (`message_id`,
-- `sent_at_ms`) of the message or reaction the call produced, in the room
-- it was sent to. `kind` is 'message' (written with the message's
-- `sending` row, before the upload) or 'reaction' (written with the
-- reaction set, after the upload). `created_at_ms` dates the key: every
-- keyed write first deletes the keys older than 28 days, hence the index.
-- Not a foreign key to `timeline_events`: a key outlives its row and still
-- answers its handle.
CREATE TABLE send_keys (
    idempotency_key  TEXT    PRIMARY KEY NOT NULL,
    room_id          TEXT    NOT NULL,           -- "uuid@domain"
    kind             TEXT    NOT NULL,           -- 'message' | 'reaction'
    message_id       TEXT    NOT NULL,
    sent_at_ms       INTEGER NOT NULL,
    created_at_ms    INTEGER NOT NULL
);
CREATE INDEX send_keys_created_at ON send_keys (created_at_ms);
