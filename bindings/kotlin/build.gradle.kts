// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Kupferdraht — Kotlin/JVM bindings (Step 20b).
//
// The `uniffi.kupferdraht` source and the bundled native `libkupferdraht.dylib`
// under src/main/{kotlin,resources} are build outputs produced by
// `build-jvm.sh`, not checked in. Run that script once before `gradle
// build` / `gradle test`.
//
// UniFFI's generated Kotlin loads the native library through JNA and
// models the async `login` export with kotlinx-coroutines.

plugins {
    kotlin("jvm") version "2.0.21"
    `java-library`
}

repositories {
    mavenCentral()
}

dependencies {
    api("net.java.dev.jna:jna:5.15.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.8.1")
    testImplementation(kotlin("test"))
}

tasks.test {
    useJUnitPlatform()
}
