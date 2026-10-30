-- Step 25b — `delivery_receipts` sibling table.
--
-- Wire's `Confirmation { type: DELIVERED }` is a per-(message, confirmer)
-- ack that the recipient's device received the message. A single
-- Confirmation event can carry one `first_message_id` plus a list of
-- `more_message_ids` — the confirmer is acknowledging an entire batch in
-- one frame. Sodium fans the batch out into one row per
-- (message_id, confirmer_user_id) tuple.
--
-- Mirror of `read_receipts` (V15): composite PK keeps redelivery
-- idempotent via `INSERT OR IGNORE`, no "newer wins" semantic, `room_id`
-- denormalised so per-room reads stay one-table. Delivery and read are
-- independent lists; a late DELIVERED never disturbs a READ.
--
-- Sodium only *sends* delivery confirmations in 1:1 rooms (group delivery
-- would be N^2 traffic for little value), but the *receive* side applies
-- whatever arrives, so a group row is possible if some other client sends
-- one.

CREATE TABLE delivery_receipts (
    message_id        TEXT    NOT NULL,
    room_id           TEXT    NOT NULL,
    confirmer_user_id TEXT    NOT NULL,
    confirmed_at_ms   INTEGER NOT NULL,
    PRIMARY KEY (message_id, confirmer_user_id)
);

CREATE INDEX idx_delivery_receipts_message ON delivery_receipts (room_id, message_id);
