-- Step 7b: introduce a `meta` table that tracks the runtime
-- column-encryption state separately from refinery's SQL-schema
-- versioning.
--
-- refinery's `refinery_schema_history` records "did we apply
-- migration V17" — strictly a SQL-DDL question. Sodium needs a
-- second sentinel to record "did we transcrypt the legacy
-- plaintext rows in `_ct` columns into AES-GCM ciphertext", which
-- is a runtime data-migration question that refinery does not own.
--
-- `schema_version` values:
--   `plaintext_columns`  — the V1–V16 legacy state. `_ct` columns
--                           hold raw plaintext, `_nonce` / `_tag` are NULL.
--   `columns_encrypted`  — every `_ct` column has been re-written
--                           with AES-GCM output keyed by the per-account
--                           DEK; `_nonce` / `_tag` are populated.
--
-- New databases get `plaintext_columns` here and the runtime
-- migration immediately advances them to `columns_encrypted` on the
-- first 7b-or-later startup. Pre-7b databases hit this same default
-- the first time V17 runs against them.
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

INSERT INTO meta (key, value) VALUES ('schema_version', 'plaintext_columns')
    ON CONFLICT(key) DO NOTHING;
