# Step 7 — Security baseline: per-field encryption + session-resume

> **Status:** complete — 7a–7e merged 2026-04-29 → 2026-05-03; smoke 5/5 against Wire production. See `THREAT_MODEL.md`. Archived from PLAN.md.
>
> **Superseded:** Step 7's per-field DEK/KEK encryption and the
> `LockedClient`/`Client` type-split were later replaced — whole-DB
> SQLCipher (ADR-0013) and the `LockedClient` removal (storage-1h). This
> document records Step 7 as originally built; its body is not updated to
> the current design.
>
> **Sub-steps:** 7a (DEK + KEK + AES-GCM helpers), 7b (column migration), 7c (LockedClient/Client type-split), 7d (UniFFI keystore callback), 7e (session-resume on device_id change).
>
> **Historical names:**
> - "Step 7" — current.
>
> **Branch / commit prefix:** `step/7-*`, `feat(step-7): …`, `chore(step-7): …`.

---

## Step 7 — Security baseline: per-field encryption + session-resume

Replace Steps 1–6's plaintext `_ct` / `_nonce` / `_tag`
columns with actual AES-256-GCM ciphertext, keyed by a DEK
that is KEK-wrapped in the platform keystore. Introduce
`LockedClient` vs `Client` types so the API enforces at
compile time that no decrypt happens on a locked session.
Plus close the **session-resume / device-replacement** gap
surfaced during Step 6c (re-`login` produces a fresh
`device_id`; existing rooms become unreachable from the new
device because their MLS leaves still belong to the prior
identity).

Decomposed into **five sub-steps** so the crypto
primitives, the one-shot migration, the API type-split,
the mobile/Web keystore integration, and the device-
replacement self-resync flow land independently. 7a–7c are
desktop-complete by themselves; 7d is explicitly separable
and can be deferred if PoC-only delivery is acceptable; 7e
is independent of 7a–7d (it touches MLS state, not
storage encryption) but lives in Step 7 because device-
hygiene and session-lifecycle are the same security domain.

### matrix-rust-sdk analog

matrix-rust-sdk's `StateStore` and `CryptoStore` traits with
per-platform SQLite implementations. The **KEK/DEK separation**
is a Kupferdraht-specific choice per ADR-0003 — matrix-rust-sdk
uses a different encryption model (per-device Olm keys +
server-side key backup).

### Public API

```rust
pub struct LockedClient { /* ... */ }
pub struct Client { /* ... */ }

impl LockedClient {
    pub async fn unlock(
        self,
        credentials: Option<AuthCredentials>,
    ) -> Result<Client, KupferdrahtError>;
}

impl Client {
    pub fn lock(self) -> LockedClient;
}

pub enum AuthCredentials { Passphrase(String) }
pub enum AuthMode { DeviceBound, PasswordProtected, PasswordOnly }
```

- `ClientBuilder::login(...)` returns `LockedClient` starting
  from Step 7. `unlock(None)` transitions to `Client` for
  `DeviceBound` mode.
- `PasswordProtected` and `PasswordOnly` ship as stubs; full
  implementation lands with Steps 8 and 9.

### Sub-steps

| Sub-step | Scope | Keystore surface | Store additions |
|---|---|---|---|
| 7a | DEK generation (random 32 bytes, `Zeroizing`), KEK wrap on **desktop only** via `keyring-rs`, `dek.bin` persistence, AES-GCM encrypt/decrypt round-trip helpers in `storage/crypto_fields.rs`. No column migration yet. | `keyring-rs` 2.x (desktop). Mobile/Web panic with "not implemented" — covered by 7d. | `dek.bin` at `$KUPFERDRAHT_HOME/accounts/<uid>/dek.bin` |
| 7b | One-shot plaintext → ciphertext column migration. Iterates every `_ct` column populated in Steps 1–6, writes AES-GCM output into the same columns, bumps `schema_version`. Idempotent. Triggered automatically on first 7b-or-later startup with legacy data. | — | `meta.schema_version` bumped to mark completion |
| 7c | `LockedClient` vs `Client` type split. `ClientBuilder::login` returns `LockedClient`; `unlock(None)` transitions to `Client` for `DeviceBound` mode. Every Step 1–6 public method moves from `Client` to `LockedClient`-gated or stays on `Client` based on whether it needs the DEK. Compiler-enforced: no decrypt without an unlocked handle. | — | — |
| 7d | Mobile/Web KEK via UniFFI callback interface. Embedder provides a `SecureKeyStore` callback (iOS Keychain, Android Keystore, Web SubtleCrypto-unwrap via IndexedDB). `keyring-rs` path on desktop preserved. **Separable — can be deferred past PoC.** | UniFFI callback interface `SecureKeyStore` | — |
| 7e | **Session-resume on `device_id` change** — merged 2026-04-29. Two changes shipped together: (a) `ClientBuilder::login` now consults the persisted `SessionRecord::device_id` (new column `session.device_id` populated by migration V8) and, if Wire still has that client (`GET /v{N}/clients/{id}` 200), rebinds the freshly-issued access token to it instead of registering a new device — the previous behaviour burned a Wire client slot per `kupferdraht-cli login` and silently desynchronised local MLS state; (b) `MlsClient::group_belongs_to_self` / `discard_group` plus `Client::resync_existing_rooms` walk every cached room and, for any whose persisted `own_leaf` credential decodes to a prior `client_id`, drop the local state and re-join via the unchanged Step-3 external-commit path. Live-smoke 2026-04-29: 20 rooms inherited from earlier `device_id`s under the pre-7e bug, all 20 resynced cleanly, send into one previously-unreachable room recovered. Closes CHANGELOG.md §29. | — | New `device_id` column on `session`; `load_device(user_id, client_id)` is the only correct shape now — picking by `user_id` alone returned arbitrary rows. |

### Wire API v14 endpoints

7a–7d are local-only. 7e re-uses the Step 3 set:
`GET /v14/conversations/{d}/{id}/groupinfo` to fetch the
current `GroupInfo` for resync, `POST /v14/mls/commit-bundles`
to upload the resync commit. No new endpoints.

### RFC 9420 sections

7a–7d: none — encryption at rest is outside the protocol scope.
7e: §12.5 (External Commit), specifically the `resync` flavour
where the joiner's existing leaf is removed in the same commit
that adds the fresh one. openmls 0.8.1 exposes this via
`ExternalCommitBuilder::with_resync_proposal`.

### Wire Message Types

None — storage-layer change only.

### Store changes

- **Key material.** DEK generated on first unlock; wrapped
  with platform KEK (`keyring-rs` on desktop, UniFFI callback
  interface on mobile/Web). Wrapped blob persisted at
  `$KUPFERDRAHT_HOME/accounts/<uid>/dek.bin`.
- **Column migration.** Every `_ct` column previously holding
  plaintext is re-encrypted in a one-shot migration:
  decrypt-nothing, encrypt-with-new-DEK, replace. Triggered
  on first 4b-build startup.
- **openmls storage.** openmls's per-table secrets
  (signature keys, epoch secrets, ratchet tree internals)
  remain in openmls's own tables. Those tables gain the
  same `_ct` / `_nonce` / `_tag` column pattern via a
  custom wrapping `StorageProvider` layered on top of
  `openmls_sqlite_storage` — Kupferdraht intercepts writes to
  re-encrypt, reads to decrypt. Alternative: accept that
  openmls's tables remain plaintext and document the
  threat-model relaxation (CHANGELOG.md §12). Decision
  pending — evaluate during Step 7 planning based on
  openmls API ergonomics.

### Implementation notes

- See `docs/secure-storage.md` for the detailed schema
  conventions.
- KEK operations via a `SecureKeyStore` trait with per-
  platform implementations. Desktop default:
  `keyring-rs` 2.x. Mobile/Web: UniFFI callback interface
  (embedder provides). Trait shape per ADR-0003.
- Migration is idempotent. A `schema_version` row in the
  `meta` table marks completion.

### TDD starter tests

7a:
- Fresh DEK generated on first `unlock` (32 random bytes).
- DEK is KEK-wrapped via `keyring-rs` on desktop; raw DEK
  never persists to disk.
- `dek.bin` contents decrypt correctly on a second
  process start via the stored KEK handle.
- AES-GCM round-trip: `encrypt_field(plaintext, dek) →
  (ct, nonce, tag) → decrypt_field` yields identical
  plaintext.

7b:
- Running the migration on a DB populated by Steps 1–6
  re-writes every `_ct` column with ciphertext.
- `meta.schema_version` is bumped only when migration
  completes.
- Running the migration a second time is a no-op
  (idempotent).
- Interrupting mid-migration leaves the DB in a resumable
  state — no column in a half-encrypted state.

7c:
- `ClientBuilder::login` returns `LockedClient`, not
  `Client`.
- Every method that reads an encrypted column fails to
  compile when called on `LockedClient`.
- `Client::lock` returns a `LockedClient`; attempting to
  re-derive `Client` without `unlock` is a type error.

7d:
- UniFFI-defined `SecureKeyStore` callback interface: a
  Kotlin test harness wraps/unwraps a DEK via a mocked
  Android Keystore stub.
- Same round-trip on an iOS test harness via a mocked
  Keychain stub.
- Desktop path (`keyring-rs`) continues to pass 7a tests
  unchanged — 7d does not regress desktop.

7e:
- Detection: a fixture group whose stored leaf identity is
  `alice:OLD_DEVICE@example.com` is flagged as needing
  resync when an `MlsClient` for `alice:NEW_DEVICE@example.com`
  inspects its rooms.
- A no-op room (leaf identity already matches current
  `device_id`) is left untouched — `resync_existing_rooms`
  is idempotent.
- Self-resync via `ExternalCommitBuilder::with_resync_proposal`
  produces a single commit-bundle that removes the old leaf
  and adds the new one; the resulting bundle has the
  expected two-segment shape (`commit || group_info`) and
  the post-merge group enumerates only the new leaf for
  this user.
- Live regression hook: a `kupferdraht-cli rejoin-all` smoke run
  after a fresh `login` against a Kupferdraht-created room from
  a prior session leaves `room-members` showing only the
  current device for the caller.

### CLI commands

```
kupferdraht-cli migrate     [--dry-run]      # 7b
kupferdraht-cli unlock      [--passphrase-stdin]   # 7c
kupferdraht-cli lock                                # 7c
kupferdraht-cli rejoin-all  [--dry-run]      # 7e — list / drive self-resync per room
```

### Smoke test

```bash
# On a Step-1-through-6 environment with pre-4b data:
kupferdraht-cli migrate
kupferdraht-cli unlock
# every subsequent command works as before; column contents on disk
# are now AES-GCM ciphertext

# 7e regression: log in afresh, then recover existing rooms.
kupferdraht-cli login -b production -e ...      # new device_id
kupferdraht-cli rejoin-all                      # self-resync into prior rooms
kupferdraht-cli room-members --id <PRIOR-ROOM>  # leaf for this user is the new device
kupferdraht-cli send         --id <PRIOR-ROOM> -m "back online"
```

### CLI commands

**Step 7 — storage migration:**

| Command | Options |
|---|---|
| `migrate` | `--dry-run` |
| `unlock` | `--passphrase-stdin` |
| `lock` | — |
