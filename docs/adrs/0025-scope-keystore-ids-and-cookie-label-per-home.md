# ADR-0025: Scope keystore ids and the login cookie label per home with a persisted home id

**Status:** Accepted
**Date:** 2026-09-25
**Deciders:** Volker Zöpfel

## Context

Every id Kupferdraht writes into the platform keystore is keystore-wide:
`global-db`, `account-<user>`, `account-<user>-mls` (ADR-0021 — "the
id namespace is the keystore instance, not the home"), and
`kupferdraht.install-id`, the source of the Wire login label
`kupferdraht-<install-id>` (Step 41). A *home* — the base directory with the
registry, the per-account databases and the Step-85 lock files — is the
unit of a Kupferdraht installation; the keystore is a device-wide secret
store: the macOS keychain service `kupferdraht` is shared by kupferdraht-cli and
kupferdraht-tui on one user account, and every smoke home shares one
`KUPFERDRAHT_DEV_KEYSTORE` directory. Every home on one keystore therefore
shares every id, with three consequences:

1. **Label eviction (open.md gap-B.33, observations §83).** Wire keeps
   one refresh cookie per `(user, label)`. Two homes of the same user
   log in with the same label; the later login replaces the earlier
   home's `zuid`, which dies at its next `POST /access`
   (`SessionExpired`). Live 2026-09-24, both directions, deterministic.
2. **Cross-home first-key race (gap-B.34).** Two homes performing the
   first-ever creation of the same id at the same instant both `get`
   nothing, both `set`; the database created under the first key is
   orphaned. ADR-0024's lock lives in the home, the contended resource
   in the keystore — the lock cannot see the other home (ADR-0024's
   explicitly open question).
3. **Accidental key sharing.** Two homes of one account run on the same
   database key; a purge of one home's keys (Step 78, not yet landed)
   would brick the other home.

Constraints in effect: keys stay per database, raw 32 bytes, each under
its own keystore id (ADR-0021); the native lock is per home / per
account and nothing is written outside the home (ADR-0024); the browser
has one instance per origin and builds no lock (ADR-0023); runtime data
never panics (CLAUDE.md "Assertions"); no new dependency where the
standard library and the existing crates suffice (ADR-0010/0016). The
decider's directives: two processes, two homes, two users on one
machine keep working unchanged (Step 85's acceptance test), and now
additionally two homes of the *same* user on one keystore must neither
evict each other nor race a first key creation. Nothing is deployed,
but every developer and fixture home is keyed under the old ids
(LEARNINGS §5.32 — enumerate before invalidating a format; §5.20 —
fresh homes cost Wire device slots). Enumerated 2026-09-25 on the
maintainer's machine: the shared dev keystore `~/.kupferdraht-smoke-keystore`
holds legacy `account-<user>` + `-mls` entries for test users 1, 2 and
3, one `global-db` and one `kupferdraht.install-id`; homes `~/.kupferdraht-smoke-1`,
`-2`, `-3` sit on it (all three `global.db` files under the one shared
`global-db` key); `~/.kupferdraht-smoke-3b` (user 3) lost its private keystore
dir at the Step-86 cleanup and is unreadable; `~/.kupferdraht-smoke-79-1/-2`
sit on `~/.kupferdraht-smoke-79-keystore`; the default CLI home predates
Step 90 (unreadable) and the kupferdraht-tui profiles key into the macOS
keychain. Step 78 (local purge) will delete keystore entries by id and
lands after this decision.

## Options Considered

### Option A: A random per-home id persisted in `base/home.id`; every keystore id and the login label carry it; legacy keys adopted once

A 128-bit random id (32 lowercase hex, `uuid::Uuid::new_v4().simple()`)
is written once to the plaintext file `base/home.id` — an identifier,
not a secret — under Step 85's home lock, and read on every open of the
home (`KupferdrahtHome::open`, held on `KupferdrahtHome`). Keystore ids become
`kupferdraht.<home-id>.global-db`, `kupferdraht.<home-id>.account-<user>`,
`kupferdraht.<home-id>.account-<user>-mls`; the login label becomes
`kupferdraht-<home-id>` (same shape and length as the retired
`kupferdraht-<install-id>`, which Wire has accepted live since Step 41).
`kupferdraht.install-id` retires (never read or written again; the entry is
left in place). Existing homes are adopted once: when the namespaced id
is absent, the legacy id exists **and the database that key opens
already exists**, the legacy secret is copied under the namespaced id
inside the same locked critical section; the legacy entry stays,
because no home can know whether a sibling still needs it. A fresh
database always gets a fresh key. The label cannot be adopted (the slot
is Wire-side): each existing home takes its per-home label at its next
login (device reuse, Step 7e — no new device slot). On wasm the same
code path runs with the id as a raw file in the OPFS pool
(`/kupferdraht/home.id`; the pool handle's `import_db_unchecked` /
`export_db` / `contains` are synchronous) and without a lock — the byte
primitive is per target exactly as `ProcessLock` already is, the
decision logic is one path.

**Pros:**
- Closes gap-B.33 and gap-B.34 at the root and ends the accidental key
  sharing: different homes never touch the same keystore entry, so
  Step 85's per-home lock now covers every first-time creation there is.
- Survives moving or renaming the home; needs no path and therefore
  works on wasm.
- No migration of databases, no device slot spent, no re-keying: the
  keys are copied, the files untouched; one re-login per home fixes the
  label.
- One function family for the ids (`key_id`, taking the home id) serves
  open, adoption and the Step-78 purge alike.

**Cons:**
- One re-login per existing home is unavoidable for the label.
- Legacy keystore entries linger until someone deletes them by hand.
- A new plaintext file whose loss orphans every key of the home; a
  copied home directory is a clone (same id, same keys, same label —
  the §83 trap rebuilt by hand).
- Ids grow by about 40 characters (length ceilings below).

### Option B: Derive the home id from the canonical base path

No file: hash the canonicalised `base_dir`.

**Pros:**
- Nothing to create, nothing to lock, nothing to lose.

**Cons:**
- Moving or renaming the home orphans every key.
- wasm has no path — the pool root is the constant `/kupferdraht`, so every
  origin would derive the same id (harmless there, but the mechanism is
  a fiction on that target).

### Option C: Store the home id in `global.db`

**Cons:**
- Impossible: the registry's own key id must be namespaced before the
  file can be opened.

### Option D: Fix only the label (per-home label in `global.db`), keep keystore-wide key ids

**Pros:**
- Smallest change; the registry can hold the label once it is open.

**Cons:**
- Closes gap-B.33 only; gap-B.34 and the key sharing stay, and Step 78's
  purge would still brick a sibling home.

### Option E: A per-keystore-id lock in a location outside the home

**Cons:**
- Contradicts ADR-0024 ("per home, nothing written elsewhere"); there is
  no portable location that both a keychain-backed CLI and a TUI agree
  on; does nothing for the label.

### Option F: Fresh homes only, no adoption

**Cons:**
- Every fixture and developer home has to be re-created: a device slot
  per home (LEARNINGS §5.20 — user 3 was 7/7 the day before Step 86),
  and the keys of the existing databases become garbage while the
  ciphertext stays behind.

## Decision

**Option A**, with these settled inputs (the plan's H1–H6):

1. **H1 — source of the id:** `base/home.id`, 32 lowercase hex + newline,
   created exactly once under `base/kupferdraht.lock` (ADR-0024's home lock;
   fast path: read the file, lock only when it is absent, re-read under
   the lock). Written atomically (temp file → fsync → rename), so a
   crash never leaves a truncated id. A malformed file is a recoverable
   `KupferdrahtError::Storage` naming the path — never auto-repaired, because
   a regenerated id orphans every key of the home.
2. **H2 — id shape:** `kupferdraht.<home-id>.global-db`,
   `kupferdraht.<home-id>.account-<user>`, `kupferdraht.<home-id>.account-<user>-mls`;
   label `kupferdraht-<home-id>`. The `kupferdraht.` prefix is deliberate: in a
   keystore shared with other software (the keychain) and beside the
   legacy entries, the home-scoped entries are recognisable. Length
   ceilings, checked against the backends with the longest id
   (`…account-<uuid>@<domain>-mls`, 89 bytes + the domain):
   macOS keychain `kSecAttrAccount` — no documented cap; Windows
   Credential Manager via keyring 3.6.3 — `username` = the id ≤ 513
   (`CRED_MAX_USERNAME_LENGTH`), target `"<id>.kupferdraht"` ≤ 32767; Linux
   Secret Service attribute — no practical cap; IndexedDB key — no
   practical cap; `FileKeyStore` file name `<id>.key(.tmp)` ≤ 255 bytes
   → domains up to 158 bytes (dev-only store; a DNS name is ≤ 253, real
   Wire domains are far shorter); Wire login `label` — 39 characters,
   unchanged since Step 41.
3. **H3 — existing homes:** one-time key adoption as described, with
   the refinement that adoption requires the database to exist; legacy
   entries and `kupferdraht.install-id` are kept and documented as manual
   cleanup. The label is new per home at the next login; a device
   registered before this decision keeps its registration-time
   `cookie` label on the server until it is re-registered.
4. **H4 — wasm:** one code path; the id lives in the OPFS pool; no
   lock (ADR-0023); the per-origin IndexedDB keystore makes the
   namespace redundant there — kept for one contract on every target.
5. **H5 — Step 78 interplay:** `keystore::key_id` becomes
   `global_db(home_id)`, `account_db(home_id, user)`,
   `account_mls(home_id, user)`, each returning the namespaced id and
   its legacy id; the purge deletes the namespaced ids only.
6. **H6 — where the id is read:** once, when the home is resolved —
   `KupferdrahtHome::open(base_dir)` replaces `KupferdrahtHome::new` at the
   resolution sites (`open_default_global_store`, `open_default_store`,
   `ClientBuilder::{login, finalize_sso, resume}`); `install_cookie_label`
   becomes `KupferdrahtHome::cookie_label()`. Public accessors
   `Kupferdraht::home_id()` and `Kupferdraht::cookie_label()` (FFI and wasm
   mirrors) make a two-home setup diagnosable; the session row records
   the label it was logged in with (`SessionRecord.cookie_label`, a
   column that has existed since Step 1 and was always `None`), so
   `status` can show a session that predates its home's label. A
   builder without a home (in-memory session) uses an ephemeral label —
   a fresh id per builder.

## Consequences

### Positive

- Two homes of one user on one keystore keep both sessions alive across
  `/access`; two homes racing a first key creation cannot orphan a
  database; a purge of one home never touches another home's keys.
- Nothing deployed is broken: existing databases open through
  adoption, no device slot is spent, one re-login per home.
- The public surface grows by two read-only accessors; no new error
  variant, no new dependency (`uuid` is already used for the install
  id).

### Negative

- Every existing home must log in once more before it holds its own
  cookie slot; until then the old shared label is still in play
  (the pre-decision state, not a regression).
- Legacy keystore entries linger: on the dev keystore three accounts ×
  two keys + `global-db` + `kupferdraht.install-id`, on developer Macs the
  keychain items — a documented manual cleanup, because Kupferdraht cannot
  know which sibling home still needs them.
- `home.id` is one more file that must never be deleted or edited:
  losing it is equivalent to losing the keystore for that home.
  Copying a home directory clones its id; the copy shares keys and label
  with the original — do not copy homes, log in.
- A first-ever open of a fresh home can now surface `AccountLocked`
  (two first opens of the same home racing the id creation) — one more
  place where the retry contract of ADR-0024 applies.
- Keystore ids are about 40 characters longer; the dev `FileKeyStore`
  reaches the 255-byte file-name limit for domains longer than 158
  bytes (was 198).

### Neutral

- Different accounts in one home share the home id by design — the
  home is the unit, the account id keeps the keys apart.
- Two homes of different users on one keystore behave exactly as
  before (Step 85's acceptance criterion, re-pinned).
- The `kupferdraht.` prefix departs from `key_id.rs`'s earlier "no prefix,
  the keystore instance is the namespace" note; the note was right about
  the instance and wrong about the home, and the prefix costs nothing.
- On wasm the namespace carries no isolation (one origin = one home =
  one keystore) and is kept for the single contract.
- A consumer with a custom `StorageProvider` and no `base_dir` had a
  keystore-persisted label before and has an ephemeral one now; such a
  session has no persisted device either, so it never resumed a cookie
  in the first place.

## Implementation Notes

Step 93 (`docs/plan/93-home-scoped-keystore-ids.md`):

- `storage/home.rs`: `KupferdrahtHome::open(base_dir)` — create the base dir
  (native), read `home.id`, else take the home lock, re-read, generate,
  write atomically; `home_id()`, `cookie_label()`; `home_id_path()`
  (`base/home.id`). Validation: exactly 32 lowercase hex characters
  (runtime data → `Storage` error). wasm: the pool handle retained at
  `install_browser_storage` serves read / write / exists synchronously.
- `keystore/key_id.rs`: `KeyId { namespaced, legacy }` + the three
  constructors taking the home id.
- `storage::load_or_create_key_locked(home, lock, keystore, &KeyId,
  database_exists)`: fast path = namespaced key present; else under the
  lock: re-read namespaced → adopt legacy (when the database exists) →
  generate. Adoption is one `set_secret` of the namespaced id; the
  legacy entry is never deleted.
- `client/builder.rs`: `install_cookie_label` removed; the label comes
  from the resolved `KupferdrahtHome` (or an ephemeral id without a home).
- `manager/mod.rs`: `Kupferdraht` holds the `KupferdrahtHome`; `home_id()`,
  `cookie_label()`; the login persist writes the label into the session
  row.
- `ffi/manager.rs` + `wasm/manager.rs`: `home_id` / `cookie_label`
  accessors. CLI `status`: `home_id`, `cookie_label` (home) and the
  session's persisted label.
- Tests: `tests/step_93_home_scoped_keystore_ids.rs` — two homes, one
  keystore, one account (distinct ids, labels, keys; `/access` on one
  leaves the other alive); the gap-B.34 race with a gated keystore;
  adoption (legacy + database → adopted, legacy untouched; legacy
  without database → fresh key; neither → fresh key); `home.id` created
  once under the lock and stable across re-opens; malformed `home.id`
  is a typed error; a wasm pin that the id survives a second
  `Kupferdraht.open` in the same worker.
- Step 78 call sites (for the purge): `open_default_store`
  (`account_db`), `open_default_global_store` (`global_db`),
  `ClientBuilder::{login,finalize_sso}` via `assemble_pending_login` and
  `ClientBuilder::resume` (`account_mls`) — all through
  `KupferdrahtHome::open` + `key_id::*(home_id, …)`.

## References

- Builds on: ADR-0024 "Guard the native critical sections with a
  per-account advisory file lock, fail-fast" — the home lock guards the
  id creation and the adoption; this decision answers the open question
  in its Negative Consequences.
- Builds on: ADR-0021 "Adopt SQLite3MC with ChaCha20-Poly1305 as the
  whole-DB encryption engine" — one raw 32-byte key per database under
  its own keystore id; the id namespace moves from the keystore instance
  to the home.
- Related: ADR-0023 "Enforce one active Kupferdraht instance per browser
  origin" — why the wasm path needs no lock; ADR-0022 — the OPFS pool
  that holds `home.id` in the browser.
- Constrained by: ADR-0010 / ADR-0016 dependency hygiene; CLAUDE.md
  "Assertions" (a malformed `home.id` and a missing legacy key are
  runtime data).
- External: Wire's per-`(user, label)` refresh-cookie model
  (observations §83); keyring 3.6.3 `windows.rs` (`CRED_MAX_*`);
  rsqlite-vfs `DbTransfer` / `VfsFilesManager` (raw pool files);
  open.md gap-B.33 / gap-B.34; LEARNINGS §5.20, §5.32.

## Revision History

- 2026-09-25: Created as Proposed (Step-93 worktree, decision-first per
  the decider's brief: the inputs H1–H6 drafted with their defaults,
  nothing implemented before the decision)
- 2026-09-25: Accepted (Volker, in the Step-93 worktree review). The
  three spots put to him were confirmed as drafted: the H3 refinement
  "adopt a legacy key only when the database it opens exists", the
  login label recorded in the session row, and the `kupferdraht.` prefix
  with `kupferdraht.install-id` left in place.
- 2026-09-25: Implementation Notes amended to the landed shape (same
  decision): `KupferdrahtHome` has no separate `home_id_path` accessor
  (`storage/home_id.rs` owns the file name); the CLI's `sync` command
  updates the session row load-then-mutate so the label recorded at
  login survives every later write. Not a supersession.
- 2026-10-07: Same decision, one number amended (ADR-0041, the rename to
  Kupferdraht): the keystore id prefix grew from the old seven-byte one
  to `kupferdraht.`, so the dev `FileKeyStore`'s 255-byte file-name
  budget caps the account domain at 153 bytes, not 158. The body above
  keeps the figure it was decided with.
