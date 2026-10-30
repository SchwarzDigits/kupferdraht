-- Step 1a initial schema.
--
-- Three tables, one per top-level concern introduced in Step 1a:
--   * session       — access / refresh tokens, API version, issue time
--   * self_user     — user's own profile (from GET /v14/self)
--   * server_config — base URLs and backend title (singleton row)
--
-- Columns holding sensitive material carry the `_ct / _nonce / _tag`
-- triple convention from the first migration. They are written as
-- plaintext bytes in Steps 1–6 and upgraded to AES-GCM ciphertext by
-- Step 7's migration pass (see docs/secure-storage.md).

CREATE TABLE session (
    user_id                   TEXT    PRIMARY KEY NOT NULL,
    access_token_ct           BLOB    NOT NULL,
    access_token_nonce        BLOB,
    access_token_tag          BLOB,
    refresh_token_ct          BLOB    NOT NULL,
    refresh_token_nonce       BLOB,
    refresh_token_tag         BLOB,
    cookie_label              TEXT,
    api_version               INTEGER NOT NULL,
    access_token_issued_at_ms INTEGER NOT NULL
);

CREATE TABLE self_user (
    id          TEXT PRIMARY KEY NOT NULL,
    handle      TEXT,
    name        TEXT NOT NULL,
    email_ct    BLOB,
    email_nonce BLOB,
    email_tag   BLOB
);

CREATE TABLE server_config (
    id            INTEGER PRIMARY KEY CHECK (id = 1),
    api_base_url  TEXT NOT NULL,
    websocket_url TEXT NOT NULL,
    title         TEXT NOT NULL
);
