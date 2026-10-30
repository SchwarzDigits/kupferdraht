-- Engine-swap sub-step 2b: retire the per-field encryption columns.
--
-- Whole-DB encryption (SQLite3MC, sub-step 2a) protects every column at
-- rest, so the legacy `_ct / _nonce / _tag` triples are redundant: the
-- `_ct` column already holds plaintext and the `_nonce` / `_tag` columns
-- are always NULL. Rename each `_ct` column to its plain name and drop
-- the vestigial `_nonce` / `_tag` siblings. None of these columns are
-- indexed or referenced by a constraint, so the ALTERs are unconditional.

ALTER TABLE session RENAME COLUMN access_token_ct TO access_token;
ALTER TABLE session DROP COLUMN access_token_nonce;
ALTER TABLE session DROP COLUMN access_token_tag;
ALTER TABLE session RENAME COLUMN refresh_token_ct TO refresh_token;
ALTER TABLE session DROP COLUMN refresh_token_nonce;
ALTER TABLE session DROP COLUMN refresh_token_tag;

ALTER TABLE self_user RENAME COLUMN email_ct TO email;
ALTER TABLE self_user DROP COLUMN email_nonce;
ALTER TABLE self_user DROP COLUMN email_tag;

ALTER TABLE backend_mls_keys RENAME COLUMN public_key_ct TO public_key;
ALTER TABLE backend_mls_keys DROP COLUMN public_key_nonce;
ALTER TABLE backend_mls_keys DROP COLUMN public_key_tag;

ALTER TABLE rooms RENAME COLUMN name_ct TO name;
ALTER TABLE rooms DROP COLUMN name_nonce;
ALTER TABLE rooms DROP COLUMN name_tag;

ALTER TABLE room_members RENAME COLUMN display_name_ct TO display_name;
ALTER TABLE room_members DROP COLUMN display_name_nonce;
ALTER TABLE room_members DROP COLUMN display_name_tag;

ALTER TABLE timeline_events RENAME COLUMN body_ct TO body;
ALTER TABLE timeline_events DROP COLUMN body_nonce;
ALTER TABLE timeline_events DROP COLUMN body_tag;

ALTER TABLE attachments RENAME COLUMN otr_key_ct TO otr_key;

-- The `meta.schema_version` sentinel only drove the removed Step 7b
-- column migration; the table has no other use.
DROP TABLE IF EXISTS meta;
