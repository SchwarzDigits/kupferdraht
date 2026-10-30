-- Step 19 — core UserId type: unify the owner rows to the qualified id.
--
-- The owner identity is now a single `UserId` (`<localpart>@<domain>`).
-- `session.user_id` / `self_user.id` / `devices.user_id` store the qualified
-- form directly, so the separate `self_user.domain` column (added in V5 only
-- to rebuild the qualified id at resume) is obsolete. Drop it. The column is
-- not indexed or referenced by a constraint, so the ALTER is unconditional.
--
-- Pre-v1: no production data — existing dev DBs are recreated on the next
-- login, which writes the qualified form into the owner columns.

ALTER TABLE self_user DROP COLUMN domain;
