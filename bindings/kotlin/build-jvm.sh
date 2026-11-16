#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Schwarz Digits KG
# SPDX-License-Identifier: GPL-3.0-only

#
# Build the Kupferdraht Kotlin/JVM package's generated source + native lib
# (Step 20b).
#
# Produces two build outputs (both git-ignored — run this once before
# `./gradlew build` / `./gradlew test`, and again after any change to
# the exported Rust surface):
#
#   - src/main/kotlin/uniffi/...     the generated UniFFI Kotlin surface
#   - src/main/resources/<jna-platform>/libkupferdraht.{dylib,so}   the native
#       library, bundled where JNA's Native.load resolves it from the classpath
#
# This sets up the host's JVM slice: macOS on a developer's machine, Linux
# on the CI runner (Step 153). Android is a separate target (AAR via
# cargo-ndk).
#
# Usage: ./build-jvm.sh [debug|release]   (default: debug)
set -euo pipefail

PROFILE="${1:-debug}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
cd "$ROOT"

RELEASE_FLAG=""
[ "$PROFILE" = "release" ] && RELEASE_FLAG="--release"

# The host's library name and JNA's resource prefix for it (one dir per
# platform JNA loads from).
case "$(uname -s)" in
    Darwin) lib="libkupferdraht.dylib"; os="darwin" ;;
    Linux)  lib="libkupferdraht.so";    os="linux" ;;
    *) echo "unsupported host OS: $(uname -s)" >&2; exit 1 ;;
esac
case "$(uname -m)" in
    arm64|aarch64) jna_platform="$os-aarch64" ;;
    x86_64)        jna_platform="$os-x86-64" ;;
    *) echo "unsupported host arch: $(uname -m)" >&2; exit 1 ;;
esac

echo ">> cargo build (host, $PROFILE)"
cargo build -p kupferdraht-core $RELEASE_FLAG

# Generate the Kotlin bindings.
gen="$(mktemp -d)"
cargo run -q -p kupferdraht-uniffi-bindgen -- generate \
    --library "target/$PROFILE/$lib" \
    --language kotlin --out-dir "$gen"

# Drop the generated package into the Gradle source set.
rm -rf "$HERE/src/main/kotlin/uniffi"
mkdir -p "$HERE/src/main/kotlin"
cp -R "$gen/uniffi" "$HERE/src/main/kotlin/uniffi"

# Bundle the native lib where JNA resolves it from the classpath
# (linux-x86-64 on the CI runner, darwin-aarch64 on an Apple-silicon Mac).
res="$HERE/src/main/resources/$jna_platform"
mkdir -p "$res"
cp "target/$PROFILE/$lib" "$res/$lib"

echo "OK: generated Kotlin + bundled $lib ($jna_platform)"
