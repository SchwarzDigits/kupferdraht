-- Step 89 — `expires_at_ms` column on `timeline_events`.
--
-- Set for a self-deleting message (Wire's `Ephemeral` wrapper,
-- `GenericMessage` field 18): the wall-clock (ms since Unix epoch)
-- at which the row's local timer elapses. V1 starts the timer at
-- dispatch/send on both paths, so the value is `sent_at_ms + ttl`.
-- Nullable: every ordinary row stays NULL.
--
-- Expiry reuses the delete tombstone (V13): the sweep clears the
-- body and stamps `redacted_at_ms`, the row stays so reply chains
-- keep resolving. `expires_at_ms` is deliberately NOT cleared by
-- the sweep — it is what distinguishes "expired" from "deleted for
-- everyone" after the fact.
--
-- The partial index backs the sweep query (`expires_at_ms <= ?`
-- over live ephemeral rows) and the resume-time re-arm scan; it
-- covers only the (few) live ephemeral rows, never the bulk of the
-- timeline.

ALTER TABLE timeline_events ADD COLUMN expires_at_ms INTEGER;

CREATE INDEX idx_timeline_events_expires_at
    ON timeline_events (expires_at_ms)
    WHERE expires_at_ms IS NOT NULL AND redacted_at_ms IS NULL;
