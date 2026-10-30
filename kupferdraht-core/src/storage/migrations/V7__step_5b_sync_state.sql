-- Step 5b — sync_state singleton.
--
-- Tracks the WebSocket consumer's progress through Wire's
-- ConsumableNotifications stream so a reconnect (after stop, crash,
-- or process restart) can resume from the last persisted point
-- without re-reading already-acked events.
--
-- Singleton row (id = 1) — this device only ever has one logical
-- event stream open at a time. The same pattern as `server_config`.
--
-- Fields:
--
-- * `sync_marker_uuid` — last `marker_id` Wire emitted in a
--   `synchronization` frame. Sent back as `?sync_marker=<id>` on
--   the next `/v14/events` connect.
-- * `last_event_id` — UUID of the last `event` frame Sodium acked.
--   Step 5b's REST backfill uses this as `?since=<id>` if the
--   marker is rejected; for now it is informational only.
-- * `last_connected_at_ms` — Unix-epoch ms of the last successful
--   handshake. Surfaced via `sodium-cli status` later for staleness
--   diagnostics.

CREATE TABLE sync_state (
    id                    INTEGER PRIMARY KEY CHECK (id = 1),
    sync_marker_uuid      TEXT,
    last_event_id         TEXT,
    last_connected_at_ms  INTEGER
);
