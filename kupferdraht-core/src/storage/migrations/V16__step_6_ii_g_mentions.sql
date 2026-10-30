-- Step 6-II-g — `mentions` sibling table.
--
-- Wire's `Text.mentions` is a repeated `Mention { start, length,
-- qualified_user_id }` carrying UTF-16 code-unit offsets into the
-- body. One row per (message_id, start, length, user_id) tuple
-- so multi-mention messages and overlapping-but-distinct mentions
-- are both representable.
--
-- Mentions belong to a specific *version* of the body; on edit
-- they are replaced wholesale (the new `Text.mentions` from the
-- inbound `MessageEdit` overrides whatever was there before).
-- Receivers therefore call DELETE-by-message_id followed by
-- per-mention INSERT in one transaction (mirrors the
-- replace-the-set pattern already used by `replace_user_reactions`).
--
-- `room_id` denormalised onto every row so per-room reads stay
-- one-table without a JOIN.

CREATE TABLE mentions (
    message_id TEXT    NOT NULL,
    room_id    TEXT    NOT NULL,
    user_id    TEXT    NOT NULL,                  -- qualified <uuid>@<domain>
    start      INTEGER NOT NULL,                  -- UTF-16 code-unit offset
    length     INTEGER NOT NULL,                  -- UTF-16 code-unit count
    PRIMARY KEY (message_id, start, length, user_id)
);

CREATE INDEX idx_mentions_message ON mentions (room_id, message_id);
