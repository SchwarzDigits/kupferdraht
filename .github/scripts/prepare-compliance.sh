#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Schwarz Digits KG
# SPDX-License-Identifier: GPL-3.0-only
#
# Pre-analyzer preparation for the Schwarz Digits compliance workflow
# (SchwarzDigits/oss-compliance full-check.yml, input `pre-analyzer-script`).
# It runs on the GitHub runner before ORT starts. The ORT container
# bind-mounts the runner's home directory at /home/ort and runs as the
# runner's user, so files placed under $HOME here are visible inside the
# container under /home/ort — paths written into configuration must use
# the container-side form.
#
# Gradle: the container's JVM is newer than the Gradle versions of the
# Kotlin bindings understand (Gradle 8.10.2 cannot read Java 25 class
# files: "Unsupported class file major version 69"). As natrium does, copy
# the runner's Temurin 17 into the home directory and pin Gradle to it —
# the daemon JVM through org.gradle.java.home, the toolchain lookup through
# installations.paths, and no auto-download, so the foojay resolver is
# never invoked.
set -euxo pipefail

RUNNER_JDK17="${JAVA_HOME_17_X64:-/usr/lib/jvm/temurin-17-jdk-amd64}"
RUNNER_SIDE="$HOME/.jdks/temurin-17"
CONTAINER_SIDE="/home/ort/.jdks/temurin-17"

mkdir -p "$HOME/.jdks"
if [ ! -d "$RUNNER_SIDE" ]; then
  cp -a "$RUNNER_JDK17" "$RUNNER_SIDE"
fi

mkdir -p "$HOME/.gradle"
cat >> "$HOME/.gradle/gradle.properties" <<PROPS
org.gradle.java.home=$CONTAINER_SIDE
org.gradle.java.installations.paths=$CONTAINER_SIDE
org.gradle.java.installations.auto-download=false
PROPS

echo "Gradle toolchain config:"
cat "$HOME/.gradle/gradle.properties"
