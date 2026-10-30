-- Step 79 — `ephemeral_ttl_ms` column on `timeline_events`.
--
-- The TTL the sender chose for an OWN ephemeral send, in milliseconds
-- (`Ephemeral.expire_after_millis`). Step 89 deliberately persists a
-- rejected ephemeral send WITHOUT `expires_at_ms` — the timer starts on
-- the backend ack only, so the body stays available for a resend — but
-- kept no record of the timer itself. A resend could then only have
-- re-sent the text as a permanent message. With the TTL on the row,
-- `Timeline::resend` re-wraps the body in the same `Ephemeral` envelope,
-- stamps `expires_at_ms = resend instant + ttl`, and arms the timer.
--
-- Nullable: every ordinary row stays NULL, inbound ephemeral rows too
-- (their `expires_at_ms` is already derived at dispatch and an inbound
-- row is never resent). No index — the column is read per row.

ALTER TABLE timeline_events ADD COLUMN ephemeral_ttl_ms INTEGER;
