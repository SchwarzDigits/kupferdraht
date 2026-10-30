-- Step 6-II-e — `reactions` sibling table.
--
-- Wire's `Reaction` is a per-(message, user) FULL SET of
-- emojis. The on-the-wire payload carries the sender's
-- current state after the toggle as a comma-separated string;
-- receivers parse the CSV and replace the sender's set in full
-- (split on `,`, trim, drop blanks).
--
-- Sodium models this with one row per (message_id, sender_user_id,
-- emoji) tuple. Toggle-on inserts; toggle-off / clear-all
-- removes. The composite PK keeps duplicate inserts idempotent
-- (Wire redelivery of the same Reaction event is a no-op).
--
-- The receive path applies a `Reaction` event by:
--   1. DELETE FROM reactions WHERE message_id = ? AND sender_user_id = ?
--   2. INSERT one row per emoji from the parsed CSV
-- both inside one transaction. This mirrors Wire's "replace the
-- user's full set" semantic without diff arithmetic.
--
-- `room_id` is denormalised onto every row so per-room queries
-- (`load_reactions_for_room`) stay one-table without a JOIN
-- against `timeline_events`.

CREATE TABLE reactions (
    message_id     TEXT    NOT NULL,
    room_id        TEXT    NOT NULL,
    sender_user_id TEXT    NOT NULL,
    emoji          TEXT    NOT NULL,
    sent_at_ms     INTEGER NOT NULL,
    PRIMARY KEY (message_id, sender_user_id, emoji)
);

CREATE INDEX idx_reactions_message ON reactions (room_id, message_id);
