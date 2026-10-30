-- Step 2 cycle 2f (resume-from-session) — self_user.domain column.
--
-- The V1 `self_user` table stored only the flat `id` (UUID).
-- Re-login without a password needs the domain too: Client's
-- resume-from-persisted-store path rebuilds the MLS identity
-- and, further down the road, the qualified-user shapes Wire's
-- newer endpoints expect.
--
-- The column is nullable so V1-vintage rows keep round-tripping;
-- the CLI writes the domain on every login from here on.

ALTER TABLE self_user ADD COLUMN domain TEXT;
