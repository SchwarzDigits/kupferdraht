-- Step 60c (ADR-0014): materialized fold cache for tasks — one row per
-- (room_id, task_id) holding the latest full state (LWW in arrival
-- order). Maintained incrementally on every task-message persist
-- (creator/created_at_ms are first-writer-wins via the upsert) and
-- rebuilt per room when a task message is redacted (the wiped body
-- drops out of the fold). Feeds Room::tasks / Client::my_tasks /
-- Client::delegated_tasks without re-folding timeline_events on every
-- read.
CREATE TABLE tasks (
    room_id           TEXT    NOT NULL,
    task_id           TEXT    NOT NULL,
    title             TEXT    NOT NULL,
    description       TEXT,
    assignee          TEXT,
    due_at_ms         INTEGER,
    status            TEXT    NOT NULL,
    priority          TEXT,
    sort_key          TEXT,
    creator_user_id   TEXT    NOT NULL,
    created_at_ms     INTEGER NOT NULL,
    updated_at_ms     INTEGER NOT NULL,
    latest_message_id TEXT    NOT NULL,
    PRIMARY KEY (room_id, task_id)
);

-- Cross-room views: "my open work" filters on (assignee, status);
-- "what I delegated" filters on creator. Per-room reads ride the
-- primary-key prefix.
CREATE INDEX idx_tasks_assignee_status ON tasks (assignee, status);
CREATE INDEX idx_tasks_creator ON tasks (creator_user_id);

-- The backfill source query (`WHERE kind = 'task' ORDER BY room_id,
-- sent_at_ms`) runs on every resume while the cache is empty — for an
-- account that never uses tasks that would be a full timeline scan
-- each time. The partial index keeps it O(task rows).
CREATE INDEX idx_timeline_events_task_rows
    ON timeline_events (room_id, sent_at_ms)
    WHERE kind = 'task';
