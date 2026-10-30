# Decouple from the GPL dependency stack

Condensed from `docs/adrs/0010-decouple-from-gpl-dependencies.md`.

- **Generate Proteus prekeys once at registration, upload, then forget them** — Ed25519 keys, Wire-exact CBOR for `POST /clients`.
- **Never run Proteus / the Double Ratchet for encryption** — all messaging is MLS.
- **Do not depend on the GPL Wire crates** (`proteus-wasm`, `proteus-traits`, `cbor-codec`) — the module uses the in-graph `ed25519-dalek` and hand-emits the fixed CBOR schema; no codec crate.
- **Keep the dependency graph GPL-free; let `cargo-deny` enforce the permissive-only allow-list.** The prekey byte format is interop (match Wire byte-for-byte), not derivation.
