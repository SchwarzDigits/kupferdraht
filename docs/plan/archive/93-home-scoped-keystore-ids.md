# Step 93 — Home-scoped keystore ids and cookie label

**Status: MERGED 2026-09-25** — commits `05940bf` (ADR-0025), `9b2094d` (the step), `b18b25a` (fix); merge commit `adb3c1d` on `main`; built on `step/93-home-scoped-keystore-ids` (worktree instance). Was: implemented 2026-09-25
(ADR-0025 accepted by Volker the same day, decision-first: H1–H6 with
their defaults plus three confirmed spots — adopt a legacy key only for
an existing database, record the label in the session row, keep the
`kupferdraht.` prefix and leave `kupferdraht.install-id` in place). Commissioned
2026-09-25 (Volker, in the Step-85 report review: home-scoped
keystores). The follow-up ADR-0024 left open; closes open.md gap-B.33
(the login label is per keystore, so a second home of the same user
evicts the first home's session) and gap-B.34 (two homes on one
keystore race the first-ever creation of the same keystore-wide id).
Lands **after Step 85** (its home lock guards the home-id creation) and
coordinates with Step 78 (the local purge deletes keystore entries by
id).

**Step 78 hand-over — the call sites the purge touches.** `key_id` is
one function family taking the home id
(`keystore::key_id::{global_db, account_db, account_mls}` → `KeyId`
with `namespaced()` + `legacy()`); the purge deletes the *namespaced*
ids only (the legacy entries are a manual cleanup — no home knows
whether a sibling still needs them). Every key is created or adopted
through `storage::load_or_create_key_locked(home, lock, keystore, &key,
&database)`; its call sites, all reached via `KupferdrahtHome::open` (which
reads `base/home.id`): `storage::open_store_in` (`account_db`, from
`open_default_store` and the default provider),
`storage::open_global_store_in` (`global_db`, from
`open_default_global_store` and the provider),
`client/builder.rs::assemble_pending_login` (`account_mls`, login +
SSO) and `ClientBuilder::resume` (`account_mls`). The purge needs
`KupferdrahtHome::open(base_dir)?.home_id()` → `key_id::account_db(id,
account)` / `account_mls(id, account)` → `delete_secret(namespaced())`,
plus the account directory and the registry row; `Kupferdraht` holds the
resolved `KupferdrahtHome` already (`manager/mod.rs`).

## Problem

Every keystore id Kupferdraht writes is keystore-wide: `global-db`,
`account-<user>`, `account-<user>-mls`, and `kupferdraht.install-id` (the
source of the login label `kupferdraht-<install-id>`, Step 41). Every home
that shares a keystore — all homes on one `KUPFERDRAHT_DEV_KEYSTORE`
directory, or CLI + TUI on one macOS user (keychain service `kupferdraht`) —
therefore shares them:

1. **Label eviction (gap-B.33, observations §83).** Wire keeps one
   refresh cookie per `(user, label)`. Two homes of the same user log
   in with the same label; the later login replaces the earlier home's
   `zuid`, which dies at its next `/access` (`SessionExpired`). Both
   directions, deterministic, live 2026-09-24.
2. **Cross-home first-key race (gap-B.34).** Two homes performing the
   first-ever creation of the same id at the same instant both `get`
   nothing, both `set`, and the database created under the first key
   is orphaned. Step 85's lock lives in the home; the contended
   resource lives in the keystore, so the lock cannot see the other
   home. (Same-home races are closed by Step 85.)
3. **Accidental key sharing.** Two homes of one account run on the
   *same* database key today — not a loss, but not an isolation
   boundary either; a `purge` of one home's keys (Step 78) would brick
   the other home.

A home is the unit of a Kupferdraht installation (registry, accounts, lock
files); the keystore is a device-wide secret store. The id namespace
has to carry the home.

## Inputs (phase 1 — decide, don't guess; ADR-0025 via `new-adr`)

- **H1 — where the home id comes from.** (a) A random id persisted in
  a plaintext file in the home (`base/home.id`), created once under
  Step 85's home lock — an identifier, not a secret. (b) Derived from
  the canonical base path — no file, but moving or renaming the home
  orphans every key, and wasm has no path. (c) Stored in `global.db` —
  impossible: the registry's own key id must be namespaced before the
  file can be opened. Default if unanswered: (a).
- **H2 — id shape.** `kupferdraht.<home-id>.global-db`,
  `kupferdraht.<home-id>.account-<user>`, `….account-<user>-mls`; the login
  label `kupferdraht-<home-id>` (replaces `kupferdraht.install-id`, which
  retires). Check the length ceilings of the keychain account field,
  the Credential Manager target name and IndexedDB keys with the
  qualified user id inside.
- **H3 — existing homes: adopt or fresh?** Nothing is deployed, but
  every developer and smoke home is keyed under the old ids
  (LEARNINGS §5.32: enumerate fixture homes **and** keystore dirs before
  invalidating a format; §5.20: fresh homes cost device slots). (a)
  Fresh homes only, documented. (b) One-time adoption: when the
  namespaced id is absent and the legacy id exists, copy the legacy
  secret under the namespaced id (delete the legacy entry only when
  no other home can still need it — it cannot be known, so keep it and
  document). The label cannot be adopted (Wire-side slot): a re-login
  takes the new per-home label. Default: (b) for keys, new label per
  home on the next login.
- **H4 — wasm.** One origin = one home (ADR-0023); the home id can be a
  file in the OPFS pool like the databases; the IndexedDB keystore is
  per origin anyway, so the namespace is redundant there but harmless —
  one code path, no cfg fork.
- **H5 — Step 78 interplay.** The purge must delete the namespaced
  ids; `key_id` becomes one function family taking the home id, used
  by open, purge and adoption alike. Whichever step lands second
  updates the other's call sites.
- **H6 — where the id is read.** Once, when the home is resolved
  (`Kupferdraht::open` / `ClientBuilder` with `base_dir`), held on
  `KupferdrahtHome` (already the single source of the layout);
  `install_cookie_label` becomes a `KupferdrahtHome` accessor.

## Scope

1. `KupferdrahtHome` carries the home id (`base/home.id`; created under the
   home lock; read on every open; legacy adoption per H3).
2. `keystore::key_id` namespaced per home; `install_cookie_label`
   replaced by the home-derived label; `kupferdraht.install-id` retired.
3. Step 85's residual test: two homes on ONE keystore racing the
   first-ever creation of the same account's key → both open, each
   under its own key (the test that could not be written in Step 85).
4. CLI: `status` prints the home id and the label in effect (a
   two-home setup becomes diagnosable); smoke replays the §83 failure
   — home 3 and home 3b of test user 3 on ONE keystore dir, both stay
   alive across a `/access` on each.
5. Docs: `docs/library.md` (Keystore + Storage layout), `docs/bindings.md`
   (foreign `SecureKeyStore` implementations see the new id shape),
   `docs/cli.md` (layout: `home.id`), `docs/ARCHITECTURE.md` §6.3 + §7,
   THREAT_MODEL note (the home id is an identifier, not a secret),
   `docs/observations.md` if the smoke shows anything, open.md
   gap-B.33 + gap-B.34 closed, ADR-0025 + `rules/0025`, ADR-0024
   revision entry (its open question is decided).

## TDD sketch

- Two homes, one keystore, one account: distinct key ids, distinct
  labels, distinct database keys; a `/access` on home A leaves home B's
  persisted session usable (mock: two labels, two cookies).
- gap-B.34 race: gated keystore, two homes, first-ever creation of the
  same account → both succeed, two keys, no `AccountLocked` (different
  homes never contend).
- Legacy adoption (H3b): a keystore holding `account-<user>` and no
  namespaced id → open succeeds, the namespaced id now holds the same
  32 bytes, the legacy entry is untouched; a keystore with neither →
  fresh key under the namespaced id only.
- `home.id` is created exactly once (two concurrent first opens of one
  fresh home → one id; the loser fails `AccountLocked` and reads the
  winner's id on retry) and survives a re-open.
- wasm pin: the home id lives in the pool and survives a second
  `Kupferdraht.open` in the same worker.
- CLI: `status` shows the home id; exit codes unchanged.

## CLI

`status` gains `home_id` and `cookie_label` lines. No new subcommand.
