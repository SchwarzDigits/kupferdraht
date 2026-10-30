# Step 42 — KMP UniFFI bindings module (`bindings/kmp`)

> **Status:** MERGED to `main` — `a16fe70`, PR #35 (squash-merge, no
> separate merge commit), 2026-06-24 (bundled with Step 41 in the same PR;
> see that step's header). Scope: extracts the gobley cargo + uniffi
> binding-generation setup out of natrium-core and into a standalone KMP
> Gradle build in this repo, so the bindings are produced next to the Rust
> code they bind; natrium-core consumes the module through a Gradle
> composite build (`includeBuild`). No smoke (build-system refactor only —
> no Rust / FFI surface / Wire change; verified by `./gradlew
> compileKotlinJvm` in this repo and in natrium, per the Verification
> section).
>
> **Branch / commit prefix:** `step/42-*`, `chore(step-42): …`.

---

## Goal

Make Kupferdraht *ship the bindings* as a real Gradle module instead of natrium
reaching into a `kupferdraht.dir` checkout and running gobley inside its own build.
Single source of truth: the binding lives where the FFI lives, so an FFI change
and its generated Kotlin/Native binding move together.

This is a **build-system refactor only** — no Rust, no FFI surface, no Wire
change. The generated `uniffi.kupferdraht` API is byte-for-byte what natrium consumed
before; only *where* it is generated moves.

## What this is not

- Not the iOS Swift/XCFramework path. gobley generates the iOS binding as
  **Kotlin/Native cinterop** over the static lib's C-ABI — no Swift, no `@objc`
  bridge. The `bindings/swift` + `bindings/ios` script path is untouched and
  orthogonal.
- Not a published artifact (yet). Consumption is local via `includeBuild`. A
  Maven/`maven-publish` path for CI / external consumers is a clean follow-up
  (same module, second consumption mode) — out of scope here.

## Layout

```
bindings/kmp/
  settings.gradle.kts     standalone build, rootProject.name = "kupferdraht-bindings"
  build.gradle.kts        kotlin-multiplatform + android-library + gobley(cargo,uniffi) + atomicfu
  gradle.properties       androidX, kotlin.native.ignoreDisabledTargets, …
  lint.xml                scoped NewApi suppression for the generated cleaner
  gradle/ + gradlew       wrapper (so the cargo helper tasks run standalone)
  native/                 git-ignored — captured link metadata (-L paths)
  local.properties        git-ignored — Android sdk.dir
```

Targets: `jvm`, `androidTarget`, `iosArm64`, `iosSimulatorArm64`.
Coordinates: `org.kupferdraht:kupferdraht-bindings:0.0.0`.

## How it builds (same model as the old in-natrium setup)

- gobley's `uniffi { generateFromLibrary() }` reads the UniFFI metadata embedded
  in Kupferdraht's **prebuilt** native lib and emits `uniffi.kupferdraht` (common + per-
  target actuals).
- gobley's `CargoBuildTask` is **disabled** on an ordinary build — the build
  consumes the cdylibs/staticlibs under `<kupferdraht>/target/<triple>/debug` plus
  link metadata restored from `native/taskOutputCache`. So a day-to-day build of
  natrium needs **no Rust toolchain**.
- `buildKupferdrahtNatives` (manual) cross-builds the native libs for every KMP
  target via cargo / cargo-ndk, then regenerates the bindings. Run it after a
  Rust change.
- `captureKupferdrahtNatives` (manual, first setup + when Kupferdraht's native deps shift)
  enables the real cargo build once to snapshot fresh `-L` link metadata into
  `native/`.

## Consumer change (natrium)

natrium-core drops the gobley plugins, the `cargo {}` / `uniffi {}` config, and
the `buildKupferdrahtNatives` / `captureKupferdrahtNatives` / `restorePrebuiltCargoCache`
tasks. Its root `settings.gradle.kts` gains:

```kotlin
includeBuild("<kupferdraht.dir>/bindings/kmp") {
    dependencySubstitution {
        substitute(module("org.kupferdraht:kupferdraht-bindings")).using(project(":"))
    }
}
```

and natrium-core declares `implementation("org.kupferdraht:kupferdraht-bindings")` in
commonMain. The `uniffi.kupferdraht.*` package then resolves from the composite
dependency; gobley's transitive runtime deps (JNA on JVM/Android, coroutines)
flow with it. `kupferdraht.dir` keeps resolving from natrium's `local.properties`,
defaulting to a sibling `../kupferdraht`.

## Verification

- `cd bindings/kmp && ./gradlew compileKotlinJvm` — the module generates +
  compiles the binding standalone.
- natrium: `./gradlew :natrium-core:compileKotlinJvm` resolves the binding via
  the composite build and compiles the facade against `uniffi.kupferdraht`.
- Android / iOS: `:natrium-core:assembleRelease` / `compileKotlinIosArm64` —
  need `native/` captured once in `bindings/kmp` (`captureKupferdrahtNatives`).
- No Rust / FFI test impact — `cargo` suite unaffected.

## Out of scope / follow-ups

- `maven-publish` for the binding (CI + external consumers).
- Retiring the older `bindings/kotlin` + `bindings/android` script builds, now
  superseded by this unified KMP module for the natrium consumer.
- Wiring all Android ABIs (currently arm64-v8a only).
