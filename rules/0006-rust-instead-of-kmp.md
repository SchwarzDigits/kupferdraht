# Rust, not Kotlin Multiplatform

Condensed from `docs/adrs/0006-rust-instead-of-kmp.md`.

- **Implement Kupferdraht in Rust 2024 with UniFFI bindings for Kotlin, Swift, WASM.**
- **Do not use Kotlin Multiplatform** — JVM/Kotlin-Native have no guaranteed zeroization primitive (threat-model G4/G6 would become convention, not compiler-enforced).
- ~~Run the server-side Dehydrated Device as the Rust binary directly — no JVM.~~ (Dropped 2026-09-25 with Step 13; no server-side deployment is planned.)
- **Enforce invariants at the type level** (e.g. `LockedSession` vs `UnlockedSession`, zeroize-on-drop).
- Do not expect direct code sharing with Kalium/Natrium; reuse is limited to protocol artifacts (`openmls`, `proteus`, MLS wire formats).
