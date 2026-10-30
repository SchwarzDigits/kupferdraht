// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

// Kupferdraht — KMP UniFFI bindings build (Step 42).
//
// Standalone Gradle build. Consumed by natrium-core through a composite build
// (`includeBuild`), and runnable on its own for the cargo helper tasks
// (`buildKupferdrahtNatives`, `captureKupferdrahtNatives`).

pluginManagement {
    repositories {
        gradlePluginPortal()
        google()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositories {
        google()
        mavenCentral()
    }
}

plugins {
    id("org.gradle.toolchains.foojay-resolver-convention") version "0.9.0"
}

rootProject.name = "kupferdraht-bindings"
