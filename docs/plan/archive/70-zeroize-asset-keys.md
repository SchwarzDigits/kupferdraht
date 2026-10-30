# Step 70 — zeroize asset key material (gap-B.21)

> **Status:** merged to `main` as `4f86dc1` (2026-08-16) — archived.
> Full gate green; no smoke (pure type-hardening refactor).
> Closes `open.md` gap-B.21 (review S17): key material held as plain
> heap/stack bytes despite the charter's compiler-enforced
> zeroization claim (THREAT_MODEL G4, ADR-0006 "invariants at the
> type level"). Reference pattern: `storage/db_key.rs::DbKey`.
>
> **Historical names:**
> - "Step 70" — proposed 2026-08-14.
>
> **Branch / commit prefix:** `step/70-*`, `feat(step-70): …`.

---

## Findings that size the step (orientation 2026-08-14)

1. **Prekeys: the review claim is half wrong.** The Ed25519
   `SigningKey`s in `encryption/prekeys.rs` are transient secrets
   (only the public halves enter the uploaded bundle — `PreKey.key`
   is CBOR of *verifying* keys, not secret), and `ed25519-dalek`
   2.2's **default `zeroize` feature already wipes them on drop**
   (`impl Drop for SigningKey` + `ZeroizeOnDrop`). What is missing
   is *explicitness*: a later `default-features = false` (say, for
   wasm size) would silently drop the wipe.
2. **Asset keys: the real gap.** `EncryptedAsset.key` and
   `EncryptedBytes.key` (`encryption/asset_crypto.rs`) hold the
   per-asset AES-256 secret as a plain `[u8; 32]` pub field; the
   fresh-key locals in `encrypt_file`/`encrypt_reader`/
   `encrypt_bytes` and the receive-side key local are plain too.
   IVs and SHA-256 digests are not secret and stay plain.

## Goal

Compiler-enforced wiping for every in-memory copy of asset key
material inside Kupferdraht's own types, and the dalek wipe made
explicit:

1. `asset_crypto`: private `Zeroizing<[u8; 32]>` fields + a
   `key_material()` accessor (DbKey pattern: private field,
   borrowing accessor); fresh-key locals in `Zeroizing`. Consumers
   (`timeline/asset_send.rs`, `asset_receive.rs`) borrow through
   the accessor / deref.
2. `Cargo.toml`: `ed25519-dalek = { version = "2", features =
   ["zeroize"] }` — the wipe survives any future
   `default-features = false`.
3. **Boundary note, not scope:** the key legitimately leaves the
   zeroized domain at two serialization boundaries — the protobuf
   `Asset.RemoteData.otr_key` field (prost `Vec<u8>`) and the
   `attachments` row (SQLCipher-encrypted at rest, ADR-0013).
   Chasing zeroization through prost/storage types is not possible
   without forking them; the module docs name the boundary.

## TDD list (red)

Zeroization-on-drop is not observable at runtime in safe Rust (no
`unsafe` — CLAUDE.md), so the contract is pinned at the type level:
`requires_zeroize_on_drop<T: ZeroizeOnDrop>` asserts against the
values the real API returns. Red mechanics per the conventions'
stub rule: the new `key_material()` accessors exist as `todo!()`
stubs, so the suite compiles and the pins fail at runtime.

1. `encrypt_file_key_material_is_compiler_wiped` (unit,
   asset_crypto) — encrypt a temp file, assert the key handle is
   `ZeroizeOnDrop`, round-trip decrypt through the handle.
2. `encrypt_bytes_key_material_is_compiler_wiped` (unit,
   asset_crypto) — same for the in-memory variant.
3. `signing_key_wipe_is_compiler_enforced` (unit, prekeys) — pins
   `SigningKey: ZeroizeOnDrop` so the dalek feature can never be
   silently dropped (green from day 0 — it documents a fact the
   build already enforces; the Cargo.toml explicitness rides the
   green phase).

Existing asset round-trip tests are the regression net for the
retype.

## Out of scope

- Receive-side storage/protobuf types (`otr_key_ct` rows, prost
  fields) — serialization boundary, see above.
- gap-B.20/22/23 (own steps / upstream).
