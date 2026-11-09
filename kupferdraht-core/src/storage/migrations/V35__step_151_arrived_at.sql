-- Step 151: `sent_at_ms` becomes the backend's time, and `arrived_at_ms` holds what
-- it used to mean: when the row entered this device's timeline, by its clock. The
-- order, the unread queries and the read marker read it. An existing row keeps its
-- position and every marker its meaning; its `sent_at_ms` stays the receive time,
-- since the backend's time was never stored.
ALTER TABLE timeline_events ADD COLUMN arrived_at_ms INTEGER NOT NULL DEFAULT 0;
UPDATE timeline_events SET arrived_at_ms = sent_at_ms;
DROP INDEX idx_timeline_events_room_id;
DROP INDEX idx_timeline_events_task_rows;
CREATE INDEX idx_timeline_events_room_id ON timeline_events (room_id, arrived_at_ms);
CREATE INDEX idx_timeline_events_task_rows
    ON timeline_events (room_id, arrived_at_ms)
    WHERE kind = 'task';
