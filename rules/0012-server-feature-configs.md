# Server feature configs & user properties

Condensed from `docs/adrs/0012-server-feature-configs.md`.

- **At login, fetch everything:** all feature configs (`GET /v14/feature-configs`) and all user properties (`GET /v14/properties` + per-key); persist each as a JSON-blob row.
- **Do not fetch configs lazily or per-feature** — "fetch all" removes the recurring "did we miss this config?" question.
- **Subscribe to `feature-config.update`, `user.properties-set`, `user.properties-delete`** and re-fetch/replace the affected row.
- **Add typed getters only for configs Kupferdraht reacts to** (V1: `file_sharing()`, `receipt_mode()`); reach everything else via the generic raw accessor.
- **Treat a config-fetch failure as non-fatal** — log at warn, continue login on safe defaults (`FileSharingPolicy::Enabled`, `ReceiptMode::On`).
- **Do not field-encrypt the feature-config or user-property tables** — configs are policy-public, properties user-public.
- **Kupferdraht reads these; it does not write them** — no `PUT`/`DELETE` on properties in V1.
