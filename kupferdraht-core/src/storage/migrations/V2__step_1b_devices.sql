-- Step 1b — devices table.
--
-- One row per (user_id, client_id) pair. The `mls_public_key` column is
-- nullable: it stays empty between Step 1b's `POST /v14/clients`
-- registration and Step 1c's MLS KeyPackage upload, then is populated
-- with the base64-encoded ECDSA_SECP256R1_SHA256 signature public key.

-- No FOREIGN KEY on user_id → self_user(id) on purpose. Devices and
-- self-user rows are populated by different sub-steps (1b writes the
-- device before the self-user row is necessarily persisted through
-- save_session). The logical relationship is maintained at the
-- application layer.

CREATE TABLE devices (
    user_id            TEXT NOT NULL,
    client_id          TEXT NOT NULL,
    mls_public_key     TEXT,
    PRIMARY KEY (user_id, client_id)
);
