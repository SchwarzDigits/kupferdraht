// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Kupferdraht — Android (AAR) bindings (Step 20b).
//
// An Android library module that packages the generated UniFFI Kotlin
// surface plus the cross-compiled native `libkupferdraht.so` (jniLibs) into
// an AAR — the form Natrium's Android layer consumes.
//
// The generated `uniffi.kupferdraht` source and the jniLibs `.so` are build
// outputs produced by `build-android.sh`, not checked in. Run that
// script once before `./gradlew assembleRelease`.

plugins {
    id("com.android.library") version "8.7.3"
    kotlin("android") version "2.0.21"
}

android {
    namespace = "io.github.schwarzdigits.kupferdraht"
    compileSdk = 35

    defaultConfig {
        minSdk = 24
        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }
}

dependencies {
    // JNA on Android needs the @aar variant — it carries JNA's own native
    // libraries per ABI, which the plain jar does not.
    api("net.java.dev.jna:jna:5.15.0@aar")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.8.1")
}
