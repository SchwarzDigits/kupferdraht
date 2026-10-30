# Step 76 — openmls 0.8.1 → 0.9.0 upgrade

> **Status:** merged to `main` as `60bbb94` (2026-09-10; red tests +
> implementation + live smoke 2026-09-09, CI green). Scope: openmls
> 0.9.0 + companions in one move (traits/rust_crypto/
> basic_credential 0.6, sqlite_storage 0.3, tls_codec 0.5, rusqlite
> 0.37, refinery 0.9); four deny.toml advisory ignores removed
> (0207/0208/0212/2024-0384 — open.md §62 CLOSED, absence
> test-pinned incl. single-version companions; 0173 remains); two
> defensive match arms for `OwnPendingCommit`/`OwnPrivateMessage`;
> committed 0.8.1-written mls.db fixtures (plain + SQLCipher,
> guarded generator) roundtrip-pinned. The commissioning-time
> storage-format trap was CORRECTED in flight (observations §73:
> JsonCodec is byte-identical in both feature states — the break is
> bincode-class); Step-74's `clear_pending_commit` assumption
> re-verified in the 0.9 source. Live smoke on the EXISTING
> 0.8.1-written smoke homes: re-login, welcome, Add-commits,
> marker/task/case, union re-broadcast all green under the 0.9
> binary. No CLI change (pure dependency step).

Commissioned 2026-09-09 (Volker: "before we do that" → after the
Step-75 archive chore). openmls 0.9.0 released 2026-08-25 — the
live trigger for the four deny.toml advisory ignores (open.md §62).

## Why now

Four signed-off `deny.toml` ignores carry the removal trigger
"when openmls > 0.8.1 is released and bumped here":

- RUSTSEC-2026-0207 / 0208 (`libcrux-sha3 0.0.8` SHAKE bugs):
  `openmls_rust_crypto 0.6` → `hpke-rs ^0.7` → `libcrux-sha3
  =0.0.10` (fixed).
- RUSTSEC-2026-0212 (`libcrux-secrets 0.0.5` aarch64 const-time):
  hpke-rs 0.7 drops `libcrux-secrets` entirely — leaves the tree.
- RUSTSEC-2024-0384 (`instant`, wasm-only): openmls 0.9 uses
  `web-time ^1.1`.

Whether RUSTSEC-2026-0173 (`proc-macro-error2` via the hax
build-time macros under libcrux-sha3) also clears is visible only in
the new lockfile — check at bump time.

## Scope

1. **Companion-crate bumps** (all move together): `openmls 0.9`,
   `openmls_traits 0.6`, `openmls_rust_crypto 0.6`,
   `openmls_basic_credential 0.6`, `openmls_sqlite_storage 0.3`,
   `tls_codec 0.5` (we use it directly — fixtures,
   `SenderExtensionIndex`). MSRV 1.91 (we build on 1.98 — fine).
2. **Storage-format compatibility:** ~~the critical trap~~
   **CORRECTED during implementation (2026-09-09, observations
   §73):** the changelog-implied "without the feature every
   installation loses its groups" did NOT hold for Kupferdraht — the
   0.7.1/0.8.1 format break lives in the serde representation of a
   few enums and only manifests in non-self-describing codecs
   (bincode); Kupferdraht's `JsonCodec` serializes variant names in both
   feature states, byte-identical, and the new default deserializer
   accepts names too. Verified by a negative probe against the
   committed 0.8.1 fixtures. The `0-8-1-storage-format` feature is
   enabled anyway (declares the storage history; future 0.9.x
   compat points), but the real contract is the fixture roundtrip
   pin. `migration-import` not needed.
3. **rusqlite unification (the real migration work):**
   `openmls_sqlite_storage 0.3` requires `rusqlite ^0.37`
   (`bundled`); we pin `rusqlite 0.32`
   (`bundled-sqlcipher-vendored-openssl`) shared across the crate —
   two rusqlite majors cannot coexist (one `libsqlite3-sys` per
   graph). Bump our rusqlite to 0.37 + `refinery` 0.8 → 0.9; verify
   SQLCipher bundling still builds on all targets and that the
   feature sets don't conflict (`bundled` vs
   `bundled-sqlcipher-vendored-openssl` — the sqlcipher variant must
   win for our DB).
4. **API adaptation:** new
   `ProcessedMessageContent::OwnPendingCommit` variant (match arms);
   whatever else the compiler surfaces.
5. **Re-verify the Step-74 batching assumption:**
   `clear_pending_commit` preserving the proposal store was verified
   in the 0.8.1 registry source; the 0.9 pending-commit rework
   touched nearby code. Re-read the 0.9 source, keep the Step-74
   fixture pins green (they would catch a change).
6. **deny.toml cleanup:** remove the four (five, if 0173 clears)
   ignore entries; update open.md §62 with the closure.

## Out of scope

- New ciphersuites (P-384, ML-DSA) — single-ciphersuite policy
  stands (gap-B.1 owns any future change).
- `targeted-messages-draft`, `AppDataUpdate` — features we do not
  use.
- gap-B.22 (staged external join) — checked, still not in 0.9.0;
  the Step-66 discard workaround stays.

## TDD list (red first)

- Persistence roundtrip pin: a 0.8.1-written mls.db (fixture file,
  generated on current main before the bump) opens under 0.9 with
  `0-8-1-storage-format` and yields the same group state
  (group_exists, epoch, own leaf).
- Step-74 pins stay green unchanged (proposal-store survival).
- Migration smoke: SQLCipher DB opens after the rusqlite 0.37 bump
  (existing V1–V26 chain applies on a fresh DB; an existing DB
  opens without re-keying).
- Full existing suite green — the 69/72/74/75 fixture families
  exercise Welcome/commit/external-proposal paths against the new
  openmls.

## Smoke

Live against schwarz-dev-wire with an EXISTING smoke home (the
persistent Step-75 homes — deliberately NOT fresh: the point is
0.8.1-written state surviving the bump), plus one fresh-room
end-to-end (welcome, send, task, case) to cover new-state writes.
