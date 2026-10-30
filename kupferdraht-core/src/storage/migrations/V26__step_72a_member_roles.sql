-- Step 72a (ADR-0018): persist the conversation-level member role so
-- the case-snapshot fold gate (72b: core fields fold only from
-- administrators) survives a restart. Neutral vocabulary
-- ('administrator' | 'member' — matrix-shaped, not Wire's wire_*
-- RoleName strings, which stay inside wire/); anything else read
-- back maps to member. Existing rows default to 'member'; the next
-- sync_metadata refreshes real roles from the conversation metadata.
ALTER TABLE room_members ADD COLUMN role TEXT NOT NULL DEFAULT 'member';
