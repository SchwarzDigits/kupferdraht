// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 45 / W2 — browser test for the persistent `IndexedDB`+`WebCrypto`
//! keystore: a secret survives a simulated reload (drop + reopen), decrypted
//! via the persisted non-extractable wrap key. wasm-only; needs a headless
//! browser (`IndexedDB` + `WebCrypto`), so it is a no-op off-wasm.
//!
//! Run: `GECKODRIVER=… CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!       cargo test -p kupferdraht-core --target wasm32-unknown-unknown --test indexeddb_keystore`
#![cfg(target_arch = "wasm32")]

use kupferdraht::keystore::{IndexedDbKeyStore, SecureKeyStore};
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
async fn keystore_persists_across_reopen() {
    // A 32-byte DB-key-shaped blob (the real payload of `load_or_create_key`).
    let secret: Vec<u8> = (0u8..32).collect();

    // `open()` establishes the wrap key first, so later persists reuse it.
    let ks = IndexedDbKeyStore::open().await.expect("open keystore");
    ks.set_secret("account-x@example.test".into(), secret.clone())
        .expect("set_secret");
    // In-memory read is immediate.
    assert_eq!(
        ks.get_secret("account-x@example.test".into()).expect("get"),
        Some(secret.clone())
    );

    // Guaranteed flush, then simulate a page reload: drop + reopen a fresh store.
    ks.persist_all().await.expect("flush");
    drop(ks);

    let ks2 = IndexedDbKeyStore::open().await.expect("reopen keystore");
    assert_eq!(
        ks2.get_secret("account-x@example.test".into())
            .expect("get after reload"),
        Some(secret),
        "secret survived reload and decrypted via the persisted WebCrypto wrap key"
    );
    // A missing id is `None`.
    assert_eq!(
        ks2.get_secret("never-set".into()).expect("get missing"),
        None
    );
}
