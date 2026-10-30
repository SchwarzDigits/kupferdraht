// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 91 — SQLite3MC in the browser (ADR-0022 decision 3): outside a
//! dedicated worker the storage cannot block (Step 111a: the VFS waits with
//! `Atomics.wait`), so the browser storage **fails closed** — a recoverable
//! error, never a silent in-memory store. Runs on the main thread on purpose
//! (`run_in_browser`).
#![cfg(target_arch = "wasm32")]

use std::sync::Arc;

use kupferdraht::KupferdrahtError;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::open_browser_storage;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
async fn browser_storage_refuses_the_main_thread() {
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    match open_browser_storage(&keystore, false).await {
        Err(KupferdrahtError::Storage(message)) => {
            assert!(
                message.contains("dedicated worker"),
                "the error names the requirement, got: {message}"
            );
        }
        Err(other) => panic!("expected KupferdrahtError::Storage, got {other:?}"),
        Ok(_) => panic!("the browser storage must not open on the main thread"),
    }
    assert!(
        kupferdraht::wasm::Kupferdraht::open(wasm_bindgen::JsValue::UNDEFINED, None)
            .await
            .is_err(),
        "Kupferdraht.open fails closed outside a dedicated worker"
    );
}
