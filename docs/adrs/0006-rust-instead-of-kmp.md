# ADR-0006: Implement Kupferdraht in Rust instead of Kotlin Multiplatform

**Status:** Accepted
**Date:** 2026-04-21
**Deciders:** Volker Zöpfel

---

## Context

Kupferdraht must run on Android, iOS, macOS, Linux, Windows, and in the browser. It must also run as a long-lived server process (for the Dehydrated Device component, see Step 9 of `PLAN.md`). Three of the libraries in Kupferdraht's immediate environment — Wire's Kalium, Schwarz Digits' Natrium, and the ecosystem of Android-focused messaging clients — are built on Kotlin Multiplatform (KMP). A straightforward reading of the situation would be to write Kupferdraht in KMP too, aligned with these neighbors.

A closer look shows that Kupferdraht's requirements differ from its neighbors' in ways that matter for the language choice.

**Memory control for cryptographic material.** Kupferdraht handles long-lived private keys, short-lived MLS epoch secrets, and plaintext message contents. The security promise in `docs/THREAT_MODEL.md` G4 (plausible zeroization) and G6 (plaintext lifecycle in a single heap) requires the library to guarantee that sensitive values are overwritten when no longer needed. The JVM and Kotlin/Native both use garbage-collected runtimes with no primitive for guaranteed, synchronous zeroization. Memory can linger in the heap, in crash dumps, in debugger inspections, long after the application thinks it is gone.

**Server-side deployment.** The Dehydrated Device is a server-side MLS client process. Running a long-lived crypto-handling process on the JVM is possible but brings GC pauses, class-loading overhead, and memory tuning complexity. Native deployment, including inside trusted execution environments (SGX, SEV, Nitro Enclaves), is straightforwardly available for native binaries and unusual for JVM processes. Kupferdraht is the only component in the ecosystem that requires a server-side deployment target; Kalium and Natrium never will.

**WebAssembly as a first-class target.** ADR-0005 established that Web delivery happens exclusively via WebAssembly, and that this is a security-driven decision, not just a performance choice. KMP has a Kotlin/Wasm target, but it has been in alpha status for years and does not have production adoption in the messaging space. The Rust-to-WASM toolchain (via `wasm-bindgen` and UniFFI's WASM support) is mature, widely used, and matches the requirements.

**Consumption of openmls.** Kupferdraht's MLS implementation is built directly on openmls, a Rust library (see ADR-0001). Calling openmls from KMP would require a Kotlin/Native FFI wrapper around a Rust library — two FFI boundaries and two language environments stacked. Calling openmls from Rust means plain function calls within the same language runtime.

**Type-level invariants.** Kupferdraht's API enforces invariants at the type level — a locked session cannot perform decryption, a conversation handle belonging to session A cannot be used with session B, a plaintext that should be zeroized on drop is enforced by the compiler. Rust's type system, `Zeroize` trait, and ownership model make these enforceable at compile time. Kotlin has some of the same expressiveness (sealed classes, value classes) but not to the same degree, and the JVM has no equivalent of `Zeroize`.

None of these requirements are absolute blockers for KMP, but together they describe a mismatch between Kupferdraht's needs and what KMP gives most easily.

---

## Options Considered

### Option A: Kotlin Multiplatform, consistent with Kalium and Natrium

Use Kotlin/JVM for Android and desktop, Kotlin/Native for iOS, Kotlin/Wasm for Web. Call openmls through a platform-specific FFI wrapper on each target.

- **Pro:** Consistent with Kalium and Natrium. The surrounding team has existing KMP expertise. Cross-project code sharing with Natrium is in principle possible.
- **Pro:** No new language to introduce to the team.
- **Con:** JVM and Kotlin/Native offer no primitive for guaranteed zeroization. The threat model's G4 and G6 guarantees become convention-based, not compiler-enforced.
- **Con:** Server-side deployment requires a JVM, bringing GC pauses and tuning complexity where a native binary would be simpler.
- **Con:** Kotlin/Wasm is not production-mature. Web delivery through it would be at the bleeding edge for an extended period.
- **Con:** Two FFI boundaries to openmls (Kotlin → native wrapper → openmls), versus one direct call in Rust. More surface area, more places for bugs at boundaries.
- **Con:** Weaker type-level invariants for API-misuse prevention.

### Option B: Rust with UniFFI-generated bindings to Kotlin, Swift, and WebAssembly

Implement Kupferdraht's core as a Rust crate. Use UniFFI to generate idiomatic bindings for Kotlin (Android, JVM desktop), Swift (iOS, macOS), and TypeScript-over-WASM (Web). Server-side deployment uses the Rust binary directly.

- **Pro:** Zeroization is compiler-enforceable via `Drop` and the `zeroize` crate. Memory lifetimes are predictable.
- **Pro:** Single codebase covers all six client platforms plus the server-side Dehydrated Device. No per-platform reimplementation.
- **Pro:** WebAssembly is a first-class Rust target with mature tooling. Web delivery is on solid ground from day one.
- **Pro:** openmls is consumed directly, with no intermediate FFI layer.
- **Pro:** Rust's type system enables the invariants we want (`LockedSession` vs `UnlockedSession` as distinct types, `Zeroize` on drop, ownership for exclusive access).
- **Con:** Team learning curve. Contributors comfortable with Kotlin/JVM need to build Rust fluency.
- **Con:** No direct code sharing with Kalium or Natrium. Kotlin consumers see UniFFI-generated bindings, which are idiomatic but not hand-written.
- **Con:** Adds the Rust toolchain to build infrastructure (rustup, cargo, cross-compilation targets).

### Option C: C or C++ with hand-written FFI bindings per platform

An older pattern, used by libsodium and some other security libraries.

- **Pro:** Maximal portability, predictable ABI, no runtime overhead.
- **Con:** Manual memory management with none of Rust's safety guarantees. Entire classes of bugs (use-after-free, double-free, buffer overflow) are back on the table.
- **Con:** Hand-written FFI bindings per platform, with no generation tool. More boilerplate, more drift.
- **Con:** Not seriously considered for a library written from scratch today. Rust is the modern equivalent with stronger safety.

---

## Decision

**We implement Kupferdraht in Rust, with UniFFI generating bindings for Kotlin, Swift, and WebAssembly (Option B).**

The memory-control argument is the decisive factor. Kupferdraht's threat model commits to zeroization as a compiler-enforceable property, and that commitment cannot be honored on the JVM or on Kotlin/Native. Everything else — server-side deployment, WASM maturity, direct openmls consumption, stronger type invariants — reinforces the same direction.

The team-learning cost is real but bounded. The core development team includes members already comfortable with Rust, and the UniFFI-generated bindings mean that downstream consumers (Natrium, CivicSeal clients) continue to see idiomatic Kotlin and Swift. Only the Kupferdraht-internal development loop requires Rust fluency.

The divergence from Kalium and Natrium is accepted. Kupferdraht serves a different architectural role than either — it is the security foundation, not the application framework — and its language choice follows from that role.

---

## Consequences

### Positive

- Zeroization of sensitive material is compiler-enforced via `Zeroize` and `Drop` implementations.
- Single Rust codebase serves all six client platforms (Android, iOS, macOS, Linux, Windows, Web) and the server-side Dehydrated Device.
- Mature WebAssembly toolchain, matching ADR-0005's Web-only-via-WASM requirement without relying on experimental language features.
- Direct use of openmls with no intermediate FFI layer.
- Type-level invariants (`LockedSession` vs `UnlockedSession`, session-bound conversation handles, zeroize-on-drop) are enforced by the compiler, not by convention.

### Negative

- Team members unfamiliar with Rust need to build fluency. Onboarding cost is real, though bounded to the Kupferdraht-internal team.
- Code cannot be shared directly with Kalium or Natrium. Both are KMP; cross-project reuse is limited to protocol-level artifacts (openmls, proteus, MLS wire formats).
- The Rust toolchain (rustup, cargo, cross-compilation targets for Android/iOS/WASM) is added to the build infrastructure.
- UniFFI-generated bindings, while idiomatic, are not identical to hand-written Kotlin or Swift. Downstream developers occasionally encounter UniFFI-specific patterns (`rustCall` wrappers, handle-based object references) that require explanation.

### Neutral

- Cross-platform reach is equivalent between Rust + UniFFI and KMP. Both approaches cover the required target platforms. The differences are in runtime characteristics, not in reach.
- The CLI test harness, by being in Rust, becomes the first consumer of each new API (see ADR-0007), but that is a consequence of this decision, not a separate reason for it.

---

## Implementation Notes

- **Rust edition:** 2024.
- **UniFFI:** version 0.28, with `async fn` support and callback interfaces for host-provided implementations (notably `SecureKeyStore`).
- **Cross-compilation targets:** Android (aarch64-linux-android, x86_64-linux-android), iOS (aarch64-apple-ios, x86_64-apple-ios for simulator), WebAssembly (wasm32-unknown-unknown), plus host targets for desktop.
- **Workspace layout:** `kupferdraht-core/` (the library crate compiled for all targets) and `kupferdraht-cli/` (the Rust test harness, native binary only).
- **Key dependencies in Rust:** `openmls`, `proteus`, `rusqlite`, `aes-gcm`, `argon2`, `bip39`, `zeroize`, `keyring-rs` (desktop only), `tokio`, `reqwest`.

Details of the CLI harness, which also lives in Rust for reasons specific to the test workflow, are covered in ADR-0007.

---

## References

- `docs/THREAT_MODEL.md` — goals G4 (plausible zeroization) and G6 (plaintext lifecycle in Rust heap) that this decision enables
- ADR-0001 — `openmls` and `proteus` as the cryptographic foundation (Rust libraries, consumed directly by virtue of this decision)
- ADR-0002 — single-Rust-crate architecture, made possible by this decision
- ADR-0005 — WebAssembly-only Web delivery, enabled by Rust's mature WASM toolchain
- ADR-0007 — the CLI test harness, also in Rust as a consequence of this decision

---

## Revision History

| Version | Date | Change |
|---|---|---|
| 1.0 | 2026-04-21 | Initial ADR |
| 1.1 | 2026-09-25 | Dehydrated Device dropped (PLAN.md Step 13): the server-side-deployment argument no longer applies. Decision unchanged — it stands on the client platforms and on compiler-enforced zeroization (G4/G6). No content change. |
