-- Step 163b: pings and attachments are stored under the library's own kind
-- names (ADR-0009). A database written before carries the backend's names
-- for them; they are rewritten here, and every other kind stays as it is.
UPDATE timeline_events SET kind = 'ping'       WHERE kind = 'knock';
UPDATE timeline_events SET kind = 'attachment' WHERE kind = 'asset';
