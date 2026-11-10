# Step 153 — The Kotlin/JVM binding built and tested in CI

**Status: commissioned 2026-11-10** (Volker), from Step 20's closing review. No CI job
builds a native binding, so their tests rot unseen: observations §93 found on
2026-10-31 that both test pins missed `onSessionExpired`, and the Kotlin test only
compiled again in Step 151, when a JDK was first on the dev machine. The Swift test's
matching lines, corrected then without Xcode, passed `swift test` on 2026-11-10 (5 of 5,
macOS slice, Xcode 27.2).

## Problem

`fmt + clippy + test` and the wasm job cover the Rust code and the browser. The
generated Kotlin and its test (`bindings/kotlin/`, `./gradlew test`) run only when
someone with a JDK runs them by hand, the Swift ones only with Xcode. The FFI parity rule (CLAUDE.md) keeps the Rust
side of the surface in step; nothing keeps the generated binding and its test in step.

## Scope

1. A CI job on `ubuntu-latest`: the pinned toolchain, a JDK 21
   (`actions/setup-java`), `bindings/kotlin/build-jvm.sh debug`, then `./gradlew test`
   in `bindings/kotlin`. On pull requests and on `main`, beside the existing jobs.
2. Its run time bounded: the cargo cache keyed for the job, the Gradle cache.
3. The supply chain of what the job downloads (the Gradle wrapper, the plugins and
   test dependencies): what `deny.toml`, ORT and Dependabot cover today, and what this
   job adds. Phase 1 measures it before anything is chosen.
4. Docs: `docs/bindings.md` (the binding is tested in CI), the platform table in
   `README.md`.

## Out of scope

- Swift: it needs a macOS runner and Xcode (NG-31).
- Android: the AAR build needs the NDK (NG-31).
- Publishing the JAR (NG-31).

## TDD sketch

- An injected break of the Kotlin test turns the job red on a pull request; the fix
  turns it green.
- No smoke: the change is CI only.
