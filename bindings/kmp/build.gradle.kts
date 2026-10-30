// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Kupferdraht — Kotlin Multiplatform UniFFI bindings (Step 42).
//
// A standalone KMP Gradle build that generates the `uniffi.kupferdraht` binding
// (commonMain `expect` + per-platform `actual`s for JVM/Android/iOS) from
// Kupferdraht's *prebuilt* native libraries, via the gobley cargo + uniffi plugins.
//
// This module used to live inside natrium-core's own build; it was extracted
// here so the bindings are produced next to the Rust code they bind. Consumers
// (natrium-core) pull it through a Gradle composite build (`includeBuild`) and
// no longer carry the gobley/cargo toolchain wiring themselves.
//
// Like the old in-natrium setup, this build does NOT compile the Rust crate on
// an ordinary build — gobley's `CargoBuildTask` is disabled and the build
// consumes the cdylibs/staticlibs that `buildKupferdrahtNatives` (cargo) places under
// `<kupferdraht>/target/<triple>/debug`, plus restored link metadata in `native/`.

import gobley.gradle.cargo.tasks.CargoBuildTask
import java.io.File
import java.util.Properties

plugins {
    kotlin("multiplatform") version "2.3.10"
    id("com.android.library") version "8.12.3"
    // gobley: generate the KMP UniFFI binding from Kupferdraht's prebuilt native
    // libraries. The cargo plugin is applied for target/cinterop wiring, but its
    // build tasks are disabled below — this module consumes Kupferdraht's native
    // artifacts, it does not compile the Rust crate (no Rust toolchain required
    // for a day-to-day build).
    id("dev.gobley.cargo") version "0.3.7"
    id("dev.gobley.uniffi") version "0.3.7"
    // The gobley-generated common code uses atomicfu for its cross-platform
    // handle map.
    id("org.jetbrains.kotlin.plugin.atomicfu") version "2.3.10"
}

group = "io.github.schwarzdigits"
version = "0.0.0"

// ── Kupferdraht source location ────────────────────────────────────────────────
// This module lives at `<kupferdraht>/bindings/kmp`; the Rust crate is two levels up
// at `<kupferdraht>/kupferdraht-core`. gobley needs the crate dir to resolve the crate
// name and the per-target output directories that hold the prebuilt libraries.
val kupferdrahtDir: File = rootProject.projectDir.parentFile.parentFile
val kupferdrahtCoreDir: File = kupferdrahtDir.resolve("kupferdraht-core")

val localProps = Properties().apply {
    val f = rootProject.file("local.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}

// NDK revision — used both for the `android { ndkVersion }` block and the
// cargo-ndk Android cross-build, so they can't drift.
val kupferdrahtNdkVersion = "29.0.14206865"
// NB: the iOS deployment target is pinned in Kupferdraht's `.cargo/config.toml`
// (`[env] IPHONEOS_DEPLOYMENT_TARGET`), the single source for every cargo
// invocation here. Nothing in this file sets it.

kotlin {
    jvmToolchain(17)

    androidTarget()
    iosArm64()
    iosSimulatorArm64()
    jvm()

    sourceSets {
        commonMain.dependencies {
            // The generated async exports (e.g. `login`) model suspension with
            // kotlinx-coroutines; expose it to consumers.
            api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2")
        }
    }
}

// Point at Kupferdraht's crate so the uniffi plugin can resolve the crate name and
// the per-target output directories that hold Kupferdraht's prebuilt libraries.
cargo {
    packageDirectory = layout.projectDirectory.dir(kupferdrahtCoreDir.absolutePath)
}

uniffi {
    // Generate the bindings from the prebuilt library's embedded UniFFI
    // metadata (proc-macro scaffolding, no UDL).
    generateFromLibrary()
}

// Consume Kupferdraht's prebuilt native libraries instead of compiling the Rust
// crate during this build:
//   - JVM/Android consume the self-contained cdylibs (.dylib/.so) under
//     kupferdraht/target/<triple>/debug.
//   - iOS consumes the static libs from kupferdraht/target/<triple>/debug and links
//     openssl/ring/sqlite from Kupferdraht's build-script output dirs. That link
//     metadata is produced by a cargo run, so it is captured once and restored
//     from native/taskOutputCache (the iOS cinterop and the JVM/Android library
//     scan read it). Re-capture it whenever Kupferdraht's native dependencies change.
val restorePrebuiltCargoCache = tasks.register<Copy>("restorePrebuiltCargoCache") {
    from(layout.projectDirectory.dir("native/taskOutputCache"))
    into(layout.buildDirectory.dir("taskOutputCache"))
}
// gobley's own cargo build is normally disabled — this module consumes Kupferdraht's
// prebuilt libs and the restored link metadata, so the day-to-day build needs no
// Rust toolchain. It is enabled only for the `captureKupferdrahtNatives` run, which
// lets gobley build the crate and emit fresh link metadata (the per-target
// `-L` search paths for ring/openssl/sqlite) that we snapshot into native/.
val capturingKupferdrahtNatives = gradle.startParameter.taskNames.any {
    it.substringAfterLast(':') == "captureKupferdrahtNatives"
}
tasks.withType<CargoBuildTask>().configureEach {
    enabled = capturingKupferdrahtNatives
    if (!capturingKupferdrahtNatives) dependsOn(restorePrebuiltCargoCache)
}

// ── buildKupferdrahtNatives ────────────────────────────────────────────────────
// Convenience task: cross-build Kupferdraht's native libs for every KMP target into
// `<kupferdraht>/target/<triple>/debug`, where the gobley consume-path picks them
// up. Run it whenever the Kupferdraht sources change; the normal build then
// regenerates the UniFFI bindings from the refreshed libs. Kept manual (not a
// build dependency) so the day-to-day build needs no Rust/NDK toolchain.
//
// Replicates the env the hand-build needs: rustup's cargo on PATH (Homebrew's
// rust lacks the cross targets) and `ANDROID_NDK_HOME` for the cargo-ndk Android
// build. (The iOS deployment target comes from Kupferdraht's `.cargo/config.toml`.)
val cargoBinDir = File(System.getProperty("user.home"), ".cargo/bin")
val androidSdkDir: String? = localProps.getProperty("sdk.dir") ?: System.getenv("ANDROID_HOME")

fun Exec.kupferdrahtCargo() {
    group = "kupferdraht"
    workingDir = kupferdrahtDir
    val existingPath = System.getenv("PATH").orEmpty()
    environment("PATH", "${cargoBinDir.absolutePath}${File.pathSeparator}$existingPath")
}

val buildKupferdrahtHost = tasks.register<Exec>("buildKupferdrahtHost") {
    description = "Build Kupferdraht's host lib (provides the UniFFI metadata bindings are generated from)."
    kupferdrahtCargo()
    commandLine("cargo", "build", "-p", "kupferdraht-core")
}

val buildKupferdrahtIosArm64 = tasks.register<Exec>("buildKupferdrahtIosArm64") {
    description = "Build Kupferdraht's static lib for aarch64-apple-ios (iOS device)."
    kupferdrahtCargo()
    commandLine("cargo", "build", "-p", "kupferdraht-core", "--target", "aarch64-apple-ios")
}

val buildKupferdrahtIosSim = tasks.register<Exec>("buildKupferdrahtIosSim") {
    description = "Build Kupferdraht's static lib for aarch64-apple-ios-sim (iOS simulator)."
    kupferdrahtCargo()
    commandLine("cargo", "build", "-p", "kupferdraht-core", "--target", "aarch64-apple-ios-sim")
}

val buildKupferdrahtAndroid = tasks.register<Exec>("buildKupferdrahtAndroid") {
    description = "Build Kupferdraht's .so for aarch64-linux-android (arm64-v8a) via cargo-ndk."
    kupferdrahtCargo()
    androidSdkDir?.let { environment("ANDROID_NDK_HOME", "$it/ndk/$kupferdrahtNdkVersion") }
    commandLine("cargo", "ndk", "-t", "arm64-v8a", "build", "-p", "kupferdraht-core")
}

tasks.register("buildKupferdrahtNatives") {
    group = "kupferdraht"
    description = "Build Kupferdraht's native libs for all KMP targets, then regenerate the UniFFI bindings."
    dependsOn(buildKupferdrahtHost, buildKupferdrahtIosArm64, buildKupferdrahtIosSim, buildKupferdrahtAndroid)
    // …then regenerate the Kotlin bindings from the refreshed libraries.
    finalizedBy("buildUniffiBindings")
}

// ── captureKupferdrahtNatives ──────────────────────────────────────────────────
// Regenerate the committed-free link metadata in native/. Run on first setup
// and whenever Kupferdraht's *native dependencies* change (a new/updated crate with
// a build script, e.g. openssl/ring/sqlite), since that shifts the per-target
// `-L` search paths and their content-hash directory names.
//
// It enables gobley's real cargo build (see `capturingKupferdrahtNatives` above),
// which compiles each target and emits the link metadata into
// build/taskOutputCache; we then snapshot that into native/taskOutputCache,
// where `restorePrebuiltCargoCache` reads it on ordinary (toolchain-free)
// builds. native/ is git-ignored — it is always regenerated, never committed.
tasks.register<Copy>("captureKupferdrahtNatives") {
    group = "kupferdraht"
    description = "Rebuild via gobley and snapshot Kupferdraht's link metadata into native/ (run when Kupferdraht's native deps change)."
    dependsOn(
        "cargoBuildIosArm64Debug",
        "cargoBuildIosSimulatorArm64Debug",
        "cargoBuildAndroidArm64Debug",
        "cargoBuildMacOSArm64Debug",
    )
    from(layout.buildDirectory.dir("taskOutputCache"))
    into(layout.projectDirectory.dir("native/taskOutputCache"))
}

// gobley exposes a consumable configuration that shares the merged uniffi.toml
// across projects (for builds with multiple uniffi packages). It is unused here
// (single uniffi module) and its under-specified variant gets wrongly selected
// by Compose Multiplatform's resource resolution, which then fails trying to
// unzip uniffi.toml as a resource archive. The configuration role is locked, so
// drop its published artifact instead — nothing consumes it in this build.
afterEvaluate {
    configurations.findByName("uniFfiConfigurationConsumable")?.outgoing?.artifacts?.clear()
}

android {
    namespace = "io.github.schwarzdigits.kupferdraht.bindings"
    compileSdk = 36
    ndkVersion = kupferdrahtNdkVersion
    defaultConfig {
        minSdk = 26
        // Restrict the Rust/Android build to arm64-v8a — only that rust target is
        // installed locally. For a release supporting all ABIs, add the rust
        // targets (armv7/x86_64/x86) and drop this filter.
        ndk { abiFilters += "arm64-v8a" }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    lint {
        // Scoped NewApi suppression for the UniFFI-generated cleaner — see lint.xml.
        lintConfig = file("lint.xml")
    }
}
