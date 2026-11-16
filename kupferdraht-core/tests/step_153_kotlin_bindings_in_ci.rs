// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 153 — the Kotlin/JVM binding is built and tested in CI: a job on
//! the Linux runner builds the library, generates the binding and runs its
//! Gradle test; the Gradle wrapper is validated and every artifact Gradle
//! downloads is pinned by its SHA-256; the doc homes say so.

use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn ci_builds_and_tests_the_kotlin_binding() {
    let ci = workspace_file(".github/workflows/ci.yml");
    for needle in [
        "actions/setup-java",
        "java-version: 21",
        "bindings/kotlin/build-jvm.sh",
        "./gradlew test",
    ] {
        assert!(ci.contains(needle), "ci.yml must contain {needle:?}");
    }
}

#[test]
fn the_gradle_wrapper_is_validated() {
    assert!(
        workspace_file(".github/workflows/ci.yml").contains("gradle/actions/wrapper-validation"),
        "CI must check the committed gradle-wrapper.jar"
    );
}

#[test]
fn every_gradle_artifact_is_pinned_by_its_checksum() {
    let metadata = workspace_file("bindings/kotlin/gradle/verification-metadata.xml");
    assert!(metadata.contains("<verify-metadata>true</verify-metadata>"));
    for (group, name, version) in [
        ("net.java.dev.jna", "jna", "5.15.0"),
        (
            "org.jetbrains.kotlinx",
            "kotlinx-coroutines-core-jvm",
            "1.8.1",
        ),
        ("org.jetbrains.kotlin", "kotlin-gradle-plugin", "2.0.21"),
    ] {
        let component = format!(r#"<component group="{group}" name="{name}" version="{version}">"#);
        assert!(
            metadata.contains(&component),
            "no checksum for {group}:{name}:{version}"
        );
    }
    assert!(metadata.contains("<sha256 value="));
}

#[test]
fn the_jvm_build_runs_on_linux() {
    let script = workspace_file("bindings/kotlin/build-jvm.sh");
    assert!(
        script.contains("linux-x86-64"),
        "JNA's resource folder on the Linux runner"
    );
    assert!(
        script.contains("libkupferdraht.so"),
        "the library's name on Linux"
    );
}

#[test]
fn the_docs_say_it_is_tested_in_ci() {
    assert!(workspace_file("docs/bindings.md").contains("Step 153"));
    assert!(workspace_file("README.md").contains("Step 153"));
    assert!(workspace_file("CLAUDE.md").contains("the Kotlin/JVM binding's test"));
}
