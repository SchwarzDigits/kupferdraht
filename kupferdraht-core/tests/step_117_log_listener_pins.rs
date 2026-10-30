// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 117 — native pins for the log listener on every surface: the wasm
//! binding, the UniFFI export, the tabs layer's declarations and worker,
//! and the embedder guides.

use std::path::Path;

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn the_wasm_binding_takes_the_listener() {
    let binding = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    for needle in ["onLog", "log_level"] {
        assert!(binding.contains(needle), "wasm/manager.rs: {needle}");
    }
    // The JS adapter is a module of its own (manager.rs is large already).
    let adapter = workspace_file("kupferdraht-core/src/wasm/logging.rs");
    for needle in ["impl LogListener for", "SendWrapper"] {
        assert!(adapter.contains(needle), "wasm/logging.rs: {needle}");
    }
}

/// The core owns the listener trait and its value types, so they carry the
/// `UniFFI` annotations themselves — as `SecureKeyStore` does — instead of
/// an `ffi/` twin.
#[test]
fn the_uniffi_surface_exports_the_listener() {
    let core = workspace_file("kupferdraht-core/src/logging.rs");
    for needle in [
        "#[uniffi::export(with_foreign)]",
        "pub trait LogListener: Send + Sync",
        "#[uniffi::export]",
        "pub fn init_logging(",
        "uniffi::Enum",
        "uniffi::Record",
    ] {
        assert!(core.contains(needle), "logging.rs: {needle}");
    }
}

#[test]
fn the_tabs_layer_passes_the_listener_on() {
    let dts = workspace_file("bindings/web/kupferdraht-tabs/index.d.ts");
    for needle in [
        "export interface LogRecord { level: 'error' | 'warn' | 'info' | 'debug' | 'trace'; target: string; message: string }",
        "logLevel?: 'error' | 'warn' | 'info' | 'debug' | 'trace';",
        "onLog?: (record: LogRecord) => void;",
    ] {
        assert!(dts.contains(needle), "index.d.ts: {needle}");
    }
    let worker = workspace_file("bindings/web/kupferdraht-tabs/worker.js");
    assert!(
        worker.contains("logLevel, onLog"),
        "worker.js passes both on"
    );
}

#[test]
fn the_guides_describe_the_listener() {
    let bindings = workspace_file("docs/bindings.md");
    for needle in [
        "onLog",
        "logLevel",
        "initLogging",
        "LogListener",
        "writes nowhere itself",
    ] {
        assert!(bindings.contains(needle), "docs/bindings.md: {needle}");
    }
    let library = workspace_file("docs/library.md");
    assert!(
        library.contains("init_logging"),
        "docs/library.md: init_logging"
    );
}
