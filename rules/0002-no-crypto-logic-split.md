# No crypto / application-logic split

Condensed from `docs/adrs/0002-no-crypto-logic-split.md`.

- **Keep cryptographic primitives and application logic in one Rust crate; expose only the application API via UniFFI.**
- **Do not split crypto and app logic across a language/FFI boundary** — plaintext crossing FFI into a GC'd heap defeats zeroization.
- **Expose application concepts (`Client`, `Room`, `Timeline`, …), never MLS primitives** on the public surface.
- Sanctioned exception: `SecureKeyStore` is host-implemented via a UniFFI callback dealing only in opaque key handles, never raw key material.
