-- Step 1c.4 — backend MLS public keys.
--
-- Wire's `GET /v14/mls/public-keys` returns the external-sender keys
-- Sodium embeds into every group's `external_senders` extension so
-- admin-driven `Remove` proposals signed by the backend validate
-- (ADR-0004, docs/external-senders-extension.md).
--
-- The (purpose, signature_scheme) pair is the logical key. Today only
-- one row is populated per backend — ("removal", "ecdsa_secp256r1_sha256")
-- matching Sodium's P-256 ciphersuite — but the composite PK leaves
-- room for the additional purposes and schemes Wire's swagger lists.
--
-- `public_key_ct` follows the `_ct / _nonce / _tag` convention: it
-- holds the raw decoded key bytes in Steps 1–6 and is upgraded to
-- AES-GCM ciphertext by Step 7's security-baseline migration.

CREATE TABLE backend_mls_keys (
    purpose           TEXT NOT NULL,
    signature_scheme  TEXT NOT NULL,
    public_key_ct     BLOB NOT NULL,
    public_key_nonce  BLOB,
    public_key_tag    BLOB,
    PRIMARY KEY (purpose, signature_scheme)
);
