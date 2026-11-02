# Rust, not Kotlin Multiplatform

Condensed from `docs/adrs/0006-rust-instead-of-kmp.md`.

- **Implement Kupferdraht in Rust 2024 with UniFFI bindings for Kotlin, Swift, WASM.**
- **Do not use Kotlin Multiplatform** — JVM/Kotlin-Native have no guaranteed zeroization primitive; the best-effort zeroization of threat-model G4/G6 (holders that are `Zeroizing` are wiped on drop) would become convention there.
- ~~Run the server-side Dehydrated Device as the Rust binary directly — no JVM.~~ (Dropped 2026-09-25 with Step 13; no server-side deployment is planned.)
- **Enforce invariants at the type level** (e.g. `Submitted` vs `Acknowledged`, session-bound handles, zeroize-on-drop; there is no `LockedSession` / `UnlockedSession` split — ADR-0006 v1.2).
- Do not expect direct code sharing with Kalium/Natrium; reuse is limited to protocol artifacts (`openmls`, `proteus`, MLS wire formats).
