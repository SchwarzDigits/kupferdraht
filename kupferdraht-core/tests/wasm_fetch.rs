// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 48 / W5 — browser smoke test proving `reqwest`'s `fetch` backend links
//! and performs a real HTTP round-trip on `wasm32-unknown-unknown` (the browser
//! does TLS; `rustls-tls`/`ring` are feature-gated off on wasm).
//!
//! The request targets the `wasm-bindgen-test` harness's **own origin**
//! (localhost) so there is no external network and no CORS — any HTTP status
//! back proves the transport (DNS/TCP/HTTP/body) worked end to end.
//!
//! Run: `GECKODRIVER=… CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!       cargo test -p kupferdraht-core --target wasm32-unknown-unknown --test wasm_fetch`
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
async fn fetch_backend_performs_a_real_round_trip() {
    let origin = web_sys::window()
        .expect("window")
        .location()
        .origin()
        .expect("origin");

    // A transport error (TLS/DNS/backend-not-linked) surfaces as `Err`; any
    // `Ok(Response)` — whatever the status — means the fetch backend ran.
    let resp = reqwest::get(&origin)
        .await
        .expect("reqwest fetch backend must reach the harness origin");

    assert!(
        resp.status().as_u16() > 0,
        "expected an HTTP status from {origin}"
    );
}
