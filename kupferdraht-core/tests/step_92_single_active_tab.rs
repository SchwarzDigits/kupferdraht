// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 92 — one active Kupferdraht instance per browser origin (ADR-0023): the
//! Web Locks lease around the storage open, in a **dedicated worker**
//! (`scripts/wasm-test.sh`).
//!
//! One scenario test, on purpose: the lease is worker-wide state and
//! wasm-bindgen-test does not run tests in source order, so the contract
//! is walked in sequence — another holder refuses the install, takeover
//! evicts that holder, the install is idempotent, and being evicted
//! ourselves fails closed. The "other tab" is played by the test's own
//! raw `navigator.locks` requests (a lock is not reentrant within one
//! agent, so a second holder in the same worker behaves like another tab).
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use kupferdraht::KupferdrahtError;
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::{
    BROWSER_INSTANCE_LEASE, BrowserEviction, SessionRecord, browser_storage_evicted,
    open_browser_storage, open_default_store, prepare_browser_database, wait_for_browser_eviction,
};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_dedicated_worker);

#[wasm_bindgen]
extern "C" {
    /// The raw Web Locks entry point — what "another tab" would call.
    #[wasm_bindgen(js_namespace = ["navigator", "locks"], js_name = request)]
    fn locks_request(name: &str, options: &JsValue, callback: &js_sys::Function)
    -> js_sys::Promise;
}

fn lock_options(if_available: bool, steal: bool) -> JsValue {
    let o = js_sys::Object::new();
    js_sys::Reflect::set(&o, &"ifAvailable".into(), &if_available.into()).expect("set");
    js_sys::Reflect::set(&o, &"steal".into(), &steal.into()).expect("set");
    o.into()
}

/// A foreign holder of the lease: the request's callback keeps the lock by
/// returning a promise that never settles. The outcome of the outer
/// request (a steal rejects it with `AbortError`) lands in `outcome`.
struct ForeignHolder {
    outcome: Rc<RefCell<Option<Result<(), String>>>>,
}

impl ForeignHolder {
    fn take(steal: bool) -> Self {
        let outcome = Rc::new(RefCell::new(None));
        let keep = Closure::once_into_js(move |_lock: JsValue| -> js_sys::Promise {
            js_sys::Promise::new(&mut |_resolve, _reject| {})
        });
        let promise = locks_request(
            BROWSER_INSTANCE_LEASE,
            &lock_options(false, steal),
            keep.unchecked_ref(),
        );
        let sink = outcome.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let result = JsFuture::from(promise).await.map(|_| ()).map_err(|e| {
                js_sys::Reflect::get(&e, &"name".into())
                    .ok()
                    .and_then(|n| n.as_string())
                    .unwrap_or_else(|| format!("{e:?}"))
            });
            *sink.borrow_mut() = Some(result);
        });
        Self { outcome }
    }

    async fn settled(&self) -> Result<(), String> {
        for _ in 0..200 {
            if let Some(r) = self.outcome.borrow().clone() {
                return r;
            }
            gloo_timers::future::TimeoutFuture::new(10).await;
        }
        panic!("the foreign holder's request never settled");
    }
}

/// Whether the lease is currently held by anyone (an `ifAvailable`
/// request hands the callback `null` when it is).
async fn lease_is_held() -> bool {
    let seen = Rc::new(RefCell::new(None));
    let sink = seen.clone();
    let probe = Closure::once_into_js(move |lock: JsValue| -> JsValue {
        *sink.borrow_mut() = Some(lock.is_null());
        JsValue::UNDEFINED
    });
    JsFuture::from(locks_request(
        BROWSER_INSTANCE_LEASE,
        &lock_options(true, false),
        probe.unchecked_ref(),
    ))
    .await
    .expect("probe request settles");
    seen.borrow().expect("probe callback ran")
}

/// A throwaway keystore: the account key is created on the open (Step 93:
/// under the home-scoped id) — the lease, not the key, is under test here.
fn fresh_keystore() -> Arc<dyn SecureKeyStore> {
    Arc::new(InMemoryKeyStore::new())
}

fn sample_session() -> SessionRecord {
    SessionRecord {
        user_id: "tab@example.test".into(),
        device_id: "dev-tab".into(),
        access_token: "token".into(),
        refresh_token: String::new(),
        cookie_label: Some("step92".into()),
        api_version: 14,
        access_token_issued_at_ms: 1_700_000_000_000,
    }
}

fn storage_error_containing<T>(result: Result<T, KupferdrahtError>, needle: &str) {
    match result {
        Err(KupferdrahtError::Storage(message)) => assert!(
            message.contains(needle),
            "expected a storage error naming {needle:?}, got: {message}"
        ),
        Err(other) => panic!("expected KupferdrahtError::Storage, got {other:?}"),
        Ok(_) => panic!("expected the {needle:?} refusal, got Ok"),
    }
}

fn evicted<T>(result: Result<T, KupferdrahtError>) {
    assert!(
        matches!(result, Err(KupferdrahtError::Evicted)),
        "expected Evicted, got {:?}",
        result.err()
    );
}

#[wasm_bindgen_test]
async fn one_active_instance_per_origin() {
    // 1. Another "tab" holds the lease → the open fails closed, and no
    //    database is touched.
    let keystore = fresh_keystore();
    let other_tab = ForeignHolder::take(false);
    assert!(lease_is_held().await, "the foreign holder has the lease");
    storage_error_containing(
        open_browser_storage(&keystore, false).await,
        "already open in another tab",
    );

    // 2. Takeover evicts the foreign holder (its request rejects with
    //    AbortError — the signal ADR-0023 relies on) and opens.
    open_browser_storage(&keystore, true)
        .await
        .expect("takeover steals the lease and opens the storage");
    assert_eq!(
        other_tab.settled().await,
        Err("AbortError".to_string()),
        "a stolen lease rejects the previous holder's request with AbortError"
    );
    assert!(lease_is_held().await, "we hold the lease now");
    assert!(!browser_storage_evicted(), "the new holder is not evicted");

    // 3. Idempotent within the holder: a second open keeps the lease.
    open_browser_storage(&keystore, false)
        .await
        .expect("a second open in the holder is a no-op");
    prepare_browser_database(Path::new(
        "kupferdraht/accounts/tab@example.test/kupferdraht.db",
    ))
    .await
    .expect("prepare the account database");
    let store = open_default_store(
        Path::new("kupferdraht"),
        &fresh_keystore(),
        "tab@example.test",
    )
    .expect("the holder opens stores");
    store
        .save_session(&sample_session())
        .await
        .expect("and writes");

    // 4. Being evicted: another "tab" steals the lease from us → the
    //    eviction future resolves with the reason, the flag flips, store
    //    operations and a re-open fail closed as `Evicted`.
    let _usurper = ForeignHolder::take(true);
    assert_eq!(wait_for_browser_eviction().await, BrowserEviction::Tab);
    assert!(browser_storage_evicted(), "evicted after a steal");
    evicted(store.save_session(&sample_session()).await);
    evicted(open_browser_storage(&keystore, false).await);
}
