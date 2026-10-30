// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 119 — a browser learns at once that another browser of the account
//! took its databases over. TDD red. Its own test binary: the eviction is
//! final for the worker. Runs in a **dedicated worker**
//! (`scripts/wasm-test.sh`).
//!
//! sqlite-remote-vfs 0.3.0 calls a listener when another instance takes a
//! server database over. Kupferdraht's listener stops the instance with the
//! reason `"device"`: `onEvicted` fires at once — not at the next write —
//! once although both databases report it, and every further store
//! operation fails as `Evicted`. The event itself needs a storage server;
//! this drives the listener Kupferdraht registers.
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use futures_util::future::Either;
use kupferdraht::storage::{BrowserEviction, database_taken_over, wait_for_browser_eviction};
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_dedicated_worker);

#[wasm_bindgen_test]
async fn a_takeover_event_stops_the_instance_with_the_reason_device() {
    let reasons: Rc<RefCell<Vec<JsValue>>> = Rc::default();
    let seen = reasons.clone();
    let on_evicted = Closure::<dyn FnMut(JsValue)>::new(move |reason: JsValue| {
        seen.borrow_mut().push(reason);
    });
    let kupferdraht = kupferdraht::wasm::Kupferdraht::open(
        JsValue::UNDEFINED,
        Some(
            on_evicted
                .as_ref()
                .unchecked_ref::<js_sys::Function>()
                .clone(),
        ),
    )
    .await
    .expect("Kupferdraht.open");

    // The VFS calls the listener once per database it holds.
    database_taken_over("kupferdraht.db");
    database_taken_over("mls.db");
    let eviction = Box::pin(wait_for_browser_eviction());
    let deadline = Box::pin(gloo_timers::future::sleep(Duration::from_secs(2)));
    let reason = match futures_util::future::select(eviction, deadline).await {
        Either::Left((reason, _)) => reason,
        Either::Right(_) => panic!("the takeover event did not stop the instance"),
    };

    assert_eq!(reason, BrowserEviction::Device);
    gloo_timers::future::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        reasons
            .borrow()
            .iter()
            .map(JsValue::as_string)
            .collect::<Vec<_>>(),
        vec![Some("device".to_owned())],
        "onEvicted(\"device\") fired once for both databases"
    );
    let Err(error) = kupferdraht.accounts().await else {
        panic!("an evicted instance refuses store operations");
    };
    assert_eq!(
        js_sys::Reflect::get(&error, &"name".into())
            .ok()
            .and_then(|n| n.as_string())
            .as_deref(),
        Some("Evicted")
    );
    drop(on_evicted);
}
