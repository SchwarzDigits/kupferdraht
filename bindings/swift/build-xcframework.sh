#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Schwarz Digits KG
# SPDX-License-Identifier: GPL-3.0-only

#
# Build the Kupferdraht Swift package's binary dependency (Step 20b).
#
# Produces two build outputs (both git-ignored — run this once before
# `swift build` / `swift test`, and again after any change to the
# exported Rust surface):
#
#   - kupferdrahtFFI.xcframework        native static lib(s) + C headers/modulemap
#   - Sources/Kupferdraht/kupferdraht.swift  the generated UniFFI Swift surface
#
# Slices: macOS (arm64) always; iOS device + simulator (arm64) are added
# when their rustup targets are installed:
#   rustup target add aarch64-apple-ios aarch64-apple-ios-sim
#
# Usage: ./build-xcframework.sh [debug|release]   (default: debug)
set -euo pipefail

PROFILE="${1:-debug}"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
cd "$ROOT"

MODULE=kupferdrahtFFI
LIB=libkupferdraht.a

# Pin the minimum OS versions the static libs are built for, matching
# `Package.swift`'s platform mins. Without this the libs inherit the
# SDK's current deployment target and refuse to run on older OSes (and
# the linker warns about the version skew). rustc and the SQLite3MC
# amalgamation's `cc` build both honour these.
export MACOSX_DEPLOYMENT_TARGET=11.0
export IPHONEOS_DEPLOYMENT_TARGET=15.0

# Single optional flag (kept as a plain string so the empty case is
# safe under `set -u` on macOS's bash 3.2).
RELEASE_FLAG=""
[ "$PROFILE" = "release" ] && RELEASE_FLAG="--release"

# macOS host slice — a normal build (implicit target) so it reuses the
# default build dir and also yields the cdylib that drives bindgen.
echo ">> cargo build (host, $PROFILE)"
cargo build -p kupferdraht-core $RELEASE_FLAG
libs=("target/$PROFILE/$LIB")

# iOS slices — staticlib only. The xcframework needs just the `.a`,
# which `cargo rustc --crate-type staticlib` produces without a link
# step (historically the cdylib also failed to link for iOS through
# openssl-sys; OpenSSL left the tree with Step 90).
for t in aarch64-apple-ios aarch64-apple-ios-sim; do
    if rustup target list --installed | grep -qx "$t"; then
        echo ">> cargo rustc $t ($PROFILE)  # staticlib only"
        cargo rustc -p kupferdraht-core --target "$t" --crate-type staticlib $RELEASE_FLAG
        libs+=("target/$t/$PROFILE/$LIB")
    else
        echo ">> skip $t (rustup target not installed)"
    fi
done

# Generate the Swift bindings from the host slice's cdylib metadata.
gen="$(mktemp -d)"
cargo run -q -p kupferdraht-uniffi-bindgen -- generate \
    --library "target/$PROFILE/libkupferdraht.dylib" \
    --language swift --out-dir "$gen"

# Stage headers (xcframework expects module.modulemap next to the .h).
hdr="$(mktemp -d)"
cp "$gen/$MODULE.h" "$hdr/"
cp "$gen/$MODULE.modulemap" "$hdr/module.modulemap"

# Pair every slice's static lib with the shared header dir.
xcargs=()
for lib in "${libs[@]}"; do
    xcargs+=(-library "$lib" -headers "$hdr")
done
rm -rf "$HERE/$MODULE.xcframework"
xcodebuild -create-xcframework "${xcargs[@]}" -output "$HERE/$MODULE.xcframework"

mkdir -p "$HERE/Sources/Kupferdraht"
cp "$gen/kupferdraht.swift" "$HERE/Sources/Kupferdraht/kupferdraht.swift"

echo "OK: $MODULE.xcframework (${#libs[@]} slice(s)) + Sources/Kupferdraht/kupferdraht.swift"
