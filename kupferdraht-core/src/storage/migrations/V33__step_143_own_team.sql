-- Step 143: the user's own team on the account snapshot, as of the last
-- login. `team_id` lowercase, NULL for a personal account; `team_name` NULL
-- too when the login could not fetch it. A row of an earlier login reads NULL
-- for both until the next login.
ALTER TABLE self_user ADD COLUMN team_id TEXT;
ALTER TABLE self_user ADD COLUMN team_name TEXT;
