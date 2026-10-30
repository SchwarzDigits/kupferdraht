# Step 20 — UniFFI bindings + packaging (Kotlin/JVM, Android, Swift iOS/mac)

> **Status:** planned (2026-06-10).
> **Sub-steps:** 20a (proc-macro scaffolding + thin vertical slice), 20b (package
> the slice: JVM JAR + iOS XCFramework/SwiftPM), 20c (full API surface), 20d
> (Android via cargo-ndk + macOS), 20e (CI + distribution + versioning).
> **Branch / commit prefix:** `step/20{letter}-*`, `feat(step-20): …`.
> **Reference:** matrix-rust-sdk-ffi (UniFFI integration + observer patterns, ADR-0009).

---

## Context

Kupferdraht's public API (`Client`, `ClientBuilder`, `Room`, `Timeline`, `SyncService`,
`EventHandler`, the `identity::UserId` / records / enums) is matrix-rust-sdk-shaped and
already `pub`, but **no foreign bindings exist yet**. The UniFFI toolchain is wired but
empty: `uniffi 0.29` (with the `tokio` feature), `crate-type = ["cdylib","staticlib","rlib"]`,
and a `build.rs` `generate_scaffolding("src/kupferdraht.udl")` — but `kupferdraht.udl` is
`namespace kupferdraht {};` (zero exported symbols) and nothing is generated or packaged.

Goal: ship **Kotlin** (Android + JVM-desktop) and **Swift** (iOS + macOS) bindings from
the single `kupferdraht-core` crate, so the KMP consumer (Natrium) can call the SDK.

## Decisions (resolved with Markus)

- **Proc-macros, not UDL.** Drop `kupferdraht.udl` + `generate_scaffolding`; use
  `uniffi::setup_scaffolding!()` + `#[derive(uniffi::…)]` / `#[uniffi::export]` (the
  matrix-rust-sdk-ffi model). One source of truth (the Rust annotations), no `.udl` to keep
  in sync.
- **Separate Kotlin + Swift bindings; Natrium does the KMP wrapping.** UniFFI emits Kotlin
  and Swift independently (matrix-rust-sdk model). Kupferdraht ships both as artifacts; **Natrium**
  owns the KMP `expect`/`actual` layer (commonMain → androidMain/jvmMain on the Kotlin
  binding, iosMain on the Swift binding). No KMP-native generator (e.g. gobley) — no
  third-party-tool risk.
- **First slice = Kotlin/JVM *and* Swift/iOS, both.** Validate the two primary toolchains
  end-to-end before expanding. Android (cargo-ndk) + macOS follow in 20d.
- **WASM is out of scope here.** ADR-0015 (web = WASM only, carried forward from 0005) runs on `wasm-bindgen` /
  `wasm-pack`, not UniFFI (whose WASM support is immature). Separate track.

## Sub-steps (TDD/validation each)

### 20a — proc-macro scaffolding + a thin vertical slice
- Swap `include_scaffolding!` + `build.rs` `generate_scaffolding` for
  `uniffi::setup_scaffolding!()`; delete `kupferdraht.udl`. Add a `uniffi-bindgen` binary
  (`uniffi::uniffi_bindgen_main()`) so bindings can be generated from the built library.
- Annotate the **minimal slice** that exercises every hard UniFFI feature exactly once:
  - an **Object** (`#[derive(uniffi::Object)]` on `Client`, `ClientBuilder`),
  - an **async method** (`#[uniffi::export(async_runtime = "tokio")]` on
    `ClientBuilder::login`) → Swift `async` / Kotlin `suspend`,
  - a **Record / value return** (`UserId` exposed as a `String` across FFI, or a small
    record),
  - an **Error** (`#[derive(uniffi::Error)]` on `KupferdrahtError`) → thrown exception,
  - a **callback interface** (`SecureKeyStore`) — foreign code (iOS Keychain / Android
    Keystore) implements it; load-bearing for mobile and a non-trivial UniFFI feature, so
    the slice must prove it.
- Goal of the slice: `ClientBuilder.login(...) -> Client`, `Client.userId() -> String`,
  with a foreign-supplied keystore. Confirm `uniffi-bindgen generate` emits **both** Kotlin
  and Swift for it.

### 20b — package + run the slice on JVM and iOS
- **Kotlin/JVM:** build the host cdylib, generate Kotlin, bundle into a JAR (UniFFI Kotlin
  is JNA-based). A tiny JVM sample logs in against a mock server and reads `userId`.
- **Swift/iOS:** build the static/cdylib for `aarch64-apple-ios`, the simulator targets
  (`aarch64/x86_64-apple-ios-sim`), and `aarch64-apple-darwin`; generate Swift + a
  module map; assemble an **XCFramework**; expose via a **SwiftPM** `Package.swift`
  (`binaryTarget` + the generated Swift). A tiny Swift sample/test drives the same flow.
- Exit criterion: the same login→userId round-trip runs from **both** a Kotlin/JVM test and
  a Swift test. The toolchain is proven before the surface grows.

### 20c — full API surface
Annotate the rest of the matrix-rust-sdk-shaped surface (this is the bulk):
- Objects: `Room`, `Timeline`, `SyncService`, `Kupferdraht`/`KupferdrahtBuilder`, `PendingClient`.
- Records / enums: `RoomMember`, `RoomType`, `RoomAccess`, `MessageBody`, `TimelineItem*`,
  `ServerConfig`, `AccountEntry`, `JoinLink`, `SyncSettings`, … via `uniffi::Record` /
  `uniffi::Enum`.
- **Observers via callback interfaces:** `EventHandler`, the `Timeline` listener /
  `Subscription` — foreign code implements the trait; mind the threading/lifetime.
- **Async** everywhere via `async_runtime = "tokio"`; **errors** via `uniffi::Error`.
- Open design questions to settle here: async-tokio runtime lifecycle (global runtime
  setup), callback-interface threading + `Subscription` drop semantics, how `UserId` /
  ids cross the boundary (flatten to `String`, per the earlier decision), and the
  `Arc<Client>` + `Weak<Self>` cycle vs UniFFI object lifetime.

### 20d — Android + macOS
- **Android:** `cargo-ndk` builds the `.so` per ABI (`arm64-v8a`, `armeabi-v7a`, `x86_64`);
  package jniLibs + the (reused) generated Kotlin into an **AAR**; publish to **Maven**
  (local first).
- **macOS:** Swift, same pipeline as iOS (already an apple target in the XCFramework) —
  fold a desktop-macOS sample in.

### 20e — CI + distribution + versioning
- Build matrix per target (apple builds need macOS runners; Android needs the NDK).
- Publish: XCFramework + SwiftPM (tag/release), AAR + Maven, JVM JAR. Version the artifacts
  in lockstep with the crate; pin the UniFFI version.
- Wire the binding build into the gate (at least: bindgen succeeds, the samples build).

## Out of scope / downstream
- **Natrium KMP wrapper** (consumer-side `expect`/`actual`) — Natrium's repo, not this one;
  Kupferdraht just ships the Kotlin + Swift artifacts and a usage doc (`docs/bindings.md`).
- **WASM/web** — separate `wasm-bindgen` track (ADR-0015).

## Risks
- **Surface size (20c)** is the real cost — the whole matrix-rust-sdk API + observers.
- **Apple toolchain** (XCFramework, code-signing, SwiftPM binaryTarget) needs a Mac + Xcode
  in CI.
- **Async-tokio + callback-interface maturity** in UniFFI 0.29 — validate early (20a) on a
  callback + async before committing the surface.
- **`#![deny(warnings)]` / `#![forbid? unsafe]`** interplay: UniFFI-generated code uses
  `#[unsafe(no_mangle)]` (CLAUDE.md already carves this exception for *generated* code).

## Verification
- 20a/20b: `uniffi-bindgen generate` emits Kotlin + Swift; the login→userId round-trip runs
  from a Kotlin/JVM test **and** a Swift test.
- Per sub-step: `cargo fmt`/`build`/`clippy -D warnings`/`test`/`deny` stay green for the
  Rust side; the generated bindings build + the samples run.
- `docs/bindings.md` written/updated alongside (per the CLAUDE.md freshness rule).

## Branch / Commit
Sub-step branches `step/20a…20e`. No `git push` (Markus). `Co-Authored-By: Claude Opus 4.8
(1M context) <noreply@anthropic.com>`.
