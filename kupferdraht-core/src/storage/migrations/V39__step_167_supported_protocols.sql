-- Step 167: the protocols the backend lists for the account as of the last
-- login, a JSON array of strings (`["mls"]` once the login declared it).
-- NULL for a row of an earlier login; it reads as an empty list until the
-- next login writes it.
ALTER TABLE self_user ADD COLUMN supported_protocols TEXT;
