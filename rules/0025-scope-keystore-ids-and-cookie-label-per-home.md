# Home-scoped keystore ids and login cookie label (`base/home.id`)

Condensed from `docs/adrs/0025-scope-keystore-ids-and-cookie-label-per-home.md`.

Accepted 2026-09-25 (Step 93 implements it). Builds on ADR-0024 (the
home lock) and ADR-0021 (one raw key per database under its own id).

- **Every keystore id carries the home id:** `kupferdraht.<home-id>.global-db`,
  `kupferdraht.<home-id>.account-<user>`, `kupferdraht.<home-id>.account-<user>-mls`;
  the Wire login label is `kupferdraht-<home-id>`. **Don't** write a
  keystore-wide id, **don't** read or write `kupferdraht.install-id` (retired,
  left in place).
- **The home id is `base/home.id`** — 32 lowercase hex, an identifier,
  not a secret — created exactly once under `base/kupferdraht.lock` (fast
  path: read; lock only when absent; re-read under the lock), written
  atomically. A malformed file is a recoverable `KupferdrahtError::Storage`;
  **never** auto-repair it (a new id orphans every key of the home).
- **Read it once when the home is resolved** (`KupferdrahtHome::open`), hold
  it on `KupferdrahtHome`; the ids come from the one `keystore::key_id`
  family taking the home id (open, adoption and the Step-78 purge alike);
  the label from `KupferdrahtHome::cookie_label`. A builder without a home
  uses an ephemeral label.
- **Adopt legacy keys once, and only for an existing database:** under
  the same lock, namespaced id absent → legacy id present → the database
  file exists → copy the secret under the namespaced id. A fresh
  database always gets a fresh key. **Don't** delete the legacy entry
  (no home knows whether a sibling still needs it); the label is never
  adopted — the next login takes the per-home label.
- **wasm:** the same code path; `home.id` is a raw file in the OPFS pool
  (`/kupferdraht/home.id`), no lock (ADR-0023); the per-origin namespace is
  redundant there and kept for the single contract.
- The login persists its label in `SessionRecord.cookie_label`; the
  public surface is `Kupferdraht::home_id` / `Kupferdraht::cookie_label` (FFI and
  wasm mirrors). **Don't** copy a home directory — the copy clones the
  id and shares keys and label with the original.
