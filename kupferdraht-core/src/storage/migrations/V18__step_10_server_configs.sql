-- Step 10b: persist Wire's server-administered configuration locally.
--
-- Two parallel tables, one per Wire endpoint:
--
-- `feature_configs`  — rows from `GET /v14/feature-configs`. One row
--                      per team-feature key (e.g. `fileSharing`,
--                      `mls`, `mlsE2EId`). `status` is nullable
--                      because not every Wire feature config carries
--                      a `status` field — some are config-only.
--                      `config_json` holds the raw `config` object as
--                      a JSON string, or `"{}"` when the feature
--                      ships status-only (see ADR-0012 §Storage
--                      shape).
--
-- `user_properties`  — rows from `GET /v14/properties` plus per-key
--                      `GET /v14/properties/{key}`. `value` is the
--                      raw response body — opaque to the storage
--                      layer (Wire mixes JSON-encoded strings like
--                      `"\"on\""` with raw numeric strings like
--                      `"1"`; typed getters parse on read).
--
-- Per ADR-0003 + ADR-0012, neither table receives field encryption.
-- These rows are server-administered policy, not user-secret content.
-- Migration number history: V17 was taken by Step 7b's
-- `meta.schema_version` migration; the next free slot is V18.

CREATE TABLE IF NOT EXISTS feature_configs (
    name              TEXT NOT NULL PRIMARY KEY,
    status            TEXT,
    config_json       TEXT NOT NULL,
    last_synced_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS user_properties (
    key               TEXT NOT NULL PRIMARY KEY,
    value             TEXT NOT NULL,
    last_synced_at_ms INTEGER NOT NULL
);
