-- Step 11d (ADR-0011): persist Wire asset attachments locally.
--
-- One row per inbound or outbound `Asset` GenericMessage. `message_id`
-- + `room_id` form the composite primary key, matching
-- `timeline_events`'s schema — every attachment is anchored to a
-- timeline message; the FK ensures cascading consistency.
--
-- `otr_key_ct` is the AES-256-GCM ciphertext of the 32-byte AES key
-- the asset's content was encrypted with. Encrypted via the per-
-- account DEK from Step 7c (ADR-0003), so a stolen `sodium.db` does
-- not surrender asset plaintext keys without also breaching the OS
-- keychain.
--
-- `cache_path` is `NULL` when the encrypted ciphertext is not in
-- Sodium's per-account cache. Set to an absolute filesystem path
-- when the encrypted blob has been downloaded (or, on send,
-- moved out of the encrypt tempfile and into the cache).
--
-- `encryption_algo` records what cipher Wire shipped (`AES_CBC` or
-- `AES_GCM`). Sodium emits only `AES_CBC`; inbound `AES_GCM` rows
-- are kept for diagnostics but cannot be decrypted (per ADR-0011
-- §AES-GCM toleration — receive-side warn-and-skip).
--
-- Migration number history: V17 = Step 7b's `meta.schema_version`.
-- V18 = Step 10b's `feature_configs` + `user_properties`. V19 is
-- next free.
CREATE TABLE attachments (
    message_id        TEXT NOT NULL,
    room_id           TEXT NOT NULL,
    mime_type         TEXT NOT NULL,
    filename          TEXT,
    size_bytes        INTEGER NOT NULL,
    sha256            BLOB NOT NULL,
    otr_key_ct        BLOB NOT NULL,
    asset_id          TEXT NOT NULL,
    asset_token       TEXT,
    asset_domain      TEXT NOT NULL,
    encryption_algo   TEXT NOT NULL,
    cache_path        TEXT,
    created_at_ms     INTEGER NOT NULL,
    PRIMARY KEY (message_id, room_id),
    -- ON DELETE NO ACTION (the SQLite default) — deleting a parent
    -- timeline_events row that has a referencing attachments row
    -- fails. Mirrors findings.md §32's V9 lesson: a CASCADE on
    -- the rooms→timeline_events edge silently wiped history on
    -- every sync_metadata. A future cleanup path that genuinely
    -- removes timeline_events rows must delete from attachments
    -- first.
    FOREIGN KEY (message_id, room_id)
        REFERENCES timeline_events(message_id, room_id)
);

CREATE INDEX idx_attachments_room ON attachments(room_id);
