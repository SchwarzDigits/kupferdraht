// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 111a — an evicted instance learns why (ADR-0023, ADR-0035). TDD
//! red. Its own test binary: the eviction is final for the worker. Runs in
//! a **dedicated worker** (`scripts/wasm-test.sh`).
//!
//! Another tab takes the origin's instance over by stealing its Web Lock;
//! `onEvicted` receives the reason `"tab"`, `wait_for_browser_eviction`
//! resolves with `BrowserEviction::Tab`, and every further store operation
//! fails as `Evicted`.
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use futures_util::future::Either;
use kupferdraht::storage::{BROWSER_INSTANCE_LEASE, BrowserEviction, wait_for_browser_eviction};
use wasm_bindgen::prelude::*;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_dedicated_worker);

/// Steal the instance lease the way a taking-over tab does.
fn steal_the_lease() {
    let steal = js_sys::Function::new_with_args(
        "name",
        "navigator.locks.request(name, { steal: true }, () => new Promise(() => {}));",
    );
    steal
        .call1(&JsValue::NULL, &JsValue::from_str(BROWSER_INSTANCE_LEASE))
        .expect("steal the lease");
}

#[wasm_bindgen_test]
async fn a_takeover_by_another_tab_evicts_with_the_reason_tab() {
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

    steal_the_lease();
    let eviction = Box::pin(wait_for_browser_eviction());
    let deadline = Box::pin(gloo_timers::future::sleep(Duration::from_secs(10)));
    let reason = match futures_util::future::select(eviction, deadline).await {
        Either::Left((reason, _)) => reason,
        Either::Right(_) => panic!("the instance was not evicted"),
    };

    assert_eq!(reason, BrowserEviction::Tab);
    gloo_timers::future::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        reasons
            .borrow()
            .iter()
            .map(JsValue::as_string)
            .collect::<Vec<_>>(),
        vec![Some("tab".to_owned())],
        "onEvicted(\"tab\") fired once"
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
