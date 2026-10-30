# Rename Sodium to Kupferdraht as a clean cut

Condensed from `docs/adrs/0041-rename-sodium-to-kupferdraht-as-a-clean-cut.md`.

Accepted 2026-10-07. Builds on `rules/0025-…` (id scheme), `0023-…`
(lease name), `0024-…` (lock file), `0026-…` (tabs module), `0040-…`
(device model); constrained by `0035-…` (frozen labels).

- **The name is Kupferdraht on every layer**, persisted identifiers
  included: `kupferdraht/v1/…` labels, `kupferdraht.db` / `kupferdraht.lock`,
  `~/.kupferdraht`, keystore prefix `kupferdraht.`, keyring service
  `kupferdraht`, lease `kupferdraht:instance`, cookie label
  `kupferdraht-<home-id>`, `KUPFERDRAHT_*`, log target `kupferdraht::`,
  device labels `Kupferdraht (<platform>)`. **Don't** keep a `sodium`
  string anywhere but the four exceptions below.
- **No migration.** A home, browser storage or keystore from before is
  wiped and logged in afresh. **Don't** add a legacy branch that reads
  `sodium…` ids or labels.
- **The labels are frozen from here on.** This rename was the one
  pre-release exception; **don't** rename a `kupferdraht/v1/…` label, a
  keystore id or a file name again without a migration ADR.
- **Four frozen artefacts keep the old string:** the nine shipped
  migration files (comments only), the step-91 fixture's marker token,
  ADR-0006's mention of the real libsodium, the SQLite3MC amalgamation.
  **Don't** edit them for the name.
- **The dev `FileKeyStore` caps the account domain at 153 bytes** (was
  158): the prefix grew by five bytes.
- **Devices registered as `Sodium (…)` are foreign to the model check**
  (ADR-0040): delete them by hand on test accounts.
- **The GitHub repository is renamed, not replaced**; `archive/sodium`
  marks the last state under the old name. **Don't** recreate
  `reklov/sodium`, it would end the redirect.
