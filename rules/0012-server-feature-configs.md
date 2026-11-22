# Server feature configs & user properties

Condensed from `docs/adrs/0012-server-feature-configs.md`.

- **At login, fetch everything:** all feature configs (`GET /v14/feature-configs`) and all user properties (`GET /v14/properties` + per-key); persist each as a JSON-blob row.
- **Do not fetch configs lazily or per-feature** — "fetch all" removes the recurring "did we miss this config?" question.
- **Subscribe to `feature-config.update`, `user.properties-set`, `user.properties-delete`** and re-fetch/replace the affected row.
- **Add typed getters only for configs Kupferdraht reacts to** (V1: `file_sharing()`, `read_receipt_setting()`); reach everything else via the generic raw accessors.
- **Treat a config-fetch failure as non-fatal** — log at warn, continue login on safe defaults (`FileSharingPolicy::Enabled`, `ReadReceiptSetting::On`).
- **Do not field-encrypt the feature-config or user-property tables** — configs are policy-public, properties user-public.
- **Kupferdraht reads these; it does not write them** — no `PUT`/`DELETE` on properties in V1.
- The public names are matrix-rust-sdk's language, not Wire's (ADR-0009 part 4, done by Step 163e): the typed getters `file_sharing()` / `read_receipt_setting()`, the raw accessors `server_settings()` / `server_setting(name)` / `account_data()` / `account_data_entry(key)` and the records `ServerSetting` / `AccountDataEntry`; **don't** add a public accessor named after Wire's feature or property names (the SQL tables keep theirs).
