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
#   - src/main/resources/<jna-arch>/libkupferdraht.dylib   the native library,
#       bundled where JNA's Native.load resolves it from the classpath
#
# This sets up the macOS (host) JVM slice. Android is a separate target
# (AAR via cargo-ndk) tracked for a later step.
#
# Usage: ./build-jvm.sh [debug|release]   (default: debug)
set -euo pipefail

PROFILE="${1:-debug}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
cd "$ROOT"

RELEASE_FLAG=""
[ "$PROFILE" = "release" ] && RELEASE_FLAG="--release"

echo ">> cargo build (host, $PROFILE)"
cargo build -p kupferdraht-core $RELEASE_FLAG

# Generate the Kotlin bindings.
gen="$(mktemp -d)"
cargo run -q -p kupferdraht-uniffi-bindgen -- generate \
    --library "target/$PROFILE/libkupferdraht.dylib" \
    --language kotlin --out-dir "$gen"

# Drop the generated package into the Gradle source set.
rm -rf "$HERE/src/main/kotlin/uniffi"
mkdir -p "$HERE/src/main/kotlin"
cp -R "$gen/uniffi" "$HERE/src/main/kotlin/uniffi"

# Bundle the native lib where JNA resolves it from the classpath. JNA's
# resource prefix for this host is the macOS arm64 name; Linux x86_64
# would be linux-x86-64, etc. (one dir per platform you ship).
arch="$(uname -m)"
case "$arch" in
    arm64|aarch64) jna_arch="darwin-aarch64" ;;
    x86_64)        jna_arch="darwin-x86-64" ;;
    *) echo "unsupported host arch: $arch" >&2; exit 1 ;;
esac
res="$HERE/src/main/resources/$jna_arch"
mkdir -p "$res"
cp "target/$PROFILE/libkupferdraht.dylib" "$res/libkupferdraht.dylib"

echo "OK: generated Kotlin + bundled libkupferdraht.dylib ($jna_arch)"
