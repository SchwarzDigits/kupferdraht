-- Step 6-II-f — `read_receipts` sibling table.
--
-- Wire's `Confirmation { type: READ }` is a per-(message, reader)
-- ack. A single Confirmation event can carry one
-- `first_message_id` plus a list of `more_message_ids` —
-- the reader is acknowledging an entire batch in one frame.
-- Sodium fans the batch out into one row per
-- (message_id, reader_user_id) tuple.
--
-- The composite PK keeps redelivery idempotent: re-receiving the
-- same READ for the same (message, reader) is a unique-constraint
-- silent skip via `INSERT OR IGNORE`. There is no "newer wins"
-- semantic — once a reader has confirmed a message, the timestamp
-- of subsequent re-confirmations doesn't matter.
--
-- `room_id` denormalised onto every row (same shape as
-- `reactions`) so per-room reads stay one-table.

CREATE TABLE read_receipts (
    message_id      TEXT    NOT NULL,
    room_id         TEXT    NOT NULL,
    reader_user_id  TEXT    NOT NULL,
    confirmed_at_ms INTEGER NOT NULL,
    PRIMARY KEY (message_id, reader_user_id)
);

CREATE INDEX idx_read_receipts_message ON read_receipts (room_id, message_id);
