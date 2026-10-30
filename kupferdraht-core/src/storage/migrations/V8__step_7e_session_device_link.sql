-- Step 7e — link the singleton `session` row to a specific device.
--
-- Before this migration the schema allowed many device rows per
-- user (`devices` PK is `(user_id, client_id)`) but the session
-- table had no FK back to a single device. Each `sodium-cli login`
-- inserted a new device row but the session was upserted in place,
-- and `load_device(user_id)` returned an arbitrary row (LIMIT 1
-- without ORDER BY — typically the oldest row by SQLite's natural
-- order). `Client::resume`, `sodium-cli status`, and the new
-- `Client::resync_existing_rooms` all silently used a stale
-- device id while the freshly-registered one stayed orphaned.
-- See findings.md §29.
--
-- Add `device_id` to `session` so the active device is explicit.
-- Backfill existing rows with the most-recently-inserted device
-- for each user — the heuristic that best matches the post-login
-- intent of the bug-era flow.

ALTER TABLE session ADD COLUMN device_id TEXT NOT NULL DEFAULT '';

UPDATE session SET device_id = (
    SELECT client_id FROM devices
    WHERE devices.user_id = session.user_id
    ORDER BY rowid DESC
    LIMIT 1
)
WHERE device_id = '';
