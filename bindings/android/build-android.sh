#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Schwarz Digits KG
# SPDX-License-Identifier: GPL-3.0-only

#
# Build the Kupferdraht Android library's generated source + native libs
# (Step 20b).
#
# Produces three build outputs (all git-ignored — run this once before
# `./gradlew assembleRelease`, and again after any change to the
# exported Rust surface):
#
#   - src/main/kotlin/uniffi/...                the generated UniFFI Kotlin surface
#   - src/main/jniLibs/<abi>/libkupferdraht.so       cross-compiled native lib(s)
#   - local.properties                          sdk.dir for AGP (machine-specific)
#
# Requires the NDK, `cargo-ndk`, and the android rust target(s):
#   sdkmanager "ndk;<version>"
#   cargo install cargo-ndk
#   rustup target add aarch64-linux-android
#
# ABI: arm64-v8a only for now. Usage: ./build-android.sh [debug|release]
# (default: release — AARs ship release).
set -euo pipefail

PROFILE="${1:-release}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
cd "$ROOT"

ANDROID_HOME="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
ANDROID_HOME="${ANDROID_HOME:-$HOME/Library/Android/sdk}"
# Newest installed NDK.
ANDROID_NDK_HOME="$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -1)"
export ANDROID_HOME ANDROID_NDK_HOME
[ -d "$ANDROID_NDK_HOME" ] || { echo "no NDK under $ANDROID_HOME/ndk — sdkmanager \"ndk;<ver>\"" >&2; exit 1; }

RELEASE_FLAG=""
[ "$PROFILE" = "release" ] && RELEASE_FLAG="--release"

# Cross-compile the cdylib for arm64-v8a; `-o` drops it into the jniLibs
# layout AGP packages (jniLibs/<abi>/libkupferdraht.so).
echo ">> cargo ndk arm64-v8a ($PROFILE)"
rm -rf "$HERE/src/main/jniLibs"
cargo ndk -t arm64-v8a -o "$HERE/src/main/jniLibs" build $RELEASE_FLAG -p kupferdraht-core

# Generate the Kotlin bindings from the host cdylib's metadata. (The
# generated surface is platform-independent; only the .so differs.)
echo ">> cargo build (host) + generate kotlin"
cargo build -p kupferdraht-core $RELEASE_FLAG
gen="$(mktemp -d)"
cargo run -q -p kupferdraht-uniffi-bindgen -- generate \
    --library "target/$PROFILE/libkupferdraht.dylib" \
    --language kotlin --out-dir "$gen"
rm -rf "$HERE/src/main/kotlin/uniffi"
mkdir -p "$HERE/src/main/kotlin"
cp -R "$gen/uniffi" "$HERE/src/main/kotlin/uniffi"

# AGP locates the SDK via local.properties (machine-specific, git-ignored).
printf 'sdk.dir=%s\n' "$ANDROID_HOME" > "$HERE/local.properties"

echo "OK: jniLibs/arm64-v8a/libkupferdraht.so + generated Kotlin (NDK: $(basename "$ANDROID_NDK_HOME"))"
