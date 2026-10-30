// swift-tools-version:5.9
// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

import PackageDescription

// Kupferdraht — Swift bindings (Step 20b).
//
// The native Rust core ships as a binary `kupferdrahtFFI.xcframework`
// (UniFFI scaffolding + the static `libkupferdraht.a`); the `Kupferdraht`
// target is the generated Swift surface that imports it. Consumers
// (e.g. Natrium's iOS layer) `import Kupferdraht`.
//
// The xcframework and the generated `Sources/Kupferdraht/kupferdraht.swift` are
// build outputs produced by `build-xcframework.sh`, not checked in.
// Run that script once before `swift build` / `swift test`.
//
// Native link dependencies of the Rust static lib (from
// `cargo rustc -- --print native-static-libs`): Security +
// CoreFoundation frameworks and iconv; `-lSystem/-lc/-lm` are implicit.
let package = Package(
    name: "Kupferdraht",
    platforms: [.macOS(.v11), .iOS(.v15)],
    products: [
        .library(name: "Kupferdraht", targets: ["Kupferdraht"]),
    ],
    targets: [
        .binaryTarget(name: "kupferdrahtFFI", path: "kupferdrahtFFI.xcframework"),
        .target(
            name: "Kupferdraht",
            dependencies: ["kupferdrahtFFI"],
            linkerSettings: [
                .linkedFramework("Security"),
                .linkedFramework("CoreFoundation"),
                .linkedLibrary("iconv"),
            ]
        ),
        .testTarget(name: "KupferdrahtTests", dependencies: ["Kupferdraht"]),
    ]
)
