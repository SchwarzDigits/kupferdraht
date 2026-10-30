// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Persistent browser [`SecureKeyStore`] (Step 45 / W2). **wasm-only.**
//!
//! The `SecureKeyStore` trait is **synchronous**, but the browser's persistent
//! storage (`IndexedDB`) and crypto (`WebCrypto`) are async and cannot be
//! blocked on. So this store bridges the gap:
//!
//! - **Preload:** [`IndexedDbKeyStore::open`] (async) loads + decrypts every
//!   stored secret into an in-memory map before the sync trait is used.
//! - **Sync reads** serve from that map.
//! - **Write-through:** `set_secret` / `delete_secret` update the map
//!   synchronously and persist in the background (`spawn_local`). Each
//!   `set_secret` write registers with `keystore::pending_writes` and reports
//!   its result, so Kupferdraht's key-creating paths await
//!   `keystore::flush_pending_writes` before they create a database with a
//!   new key (Step 104); a failed write is logged when it fails.
//!   [`IndexedDbKeyStore::persist_all`] rewrites every entry.
//!
//! **At-rest protection:** the DB keys are wrapped with a **non-extractable**
//! AES-256-GCM `WebCrypto` key (its raw material never leaves the browser's
//! crypto subsystem), so a raw `IndexedDB` dump does not yield the keys — the
//! encryption of the app data (W1c) is meaningful only because of this. The
//! wrap key itself is stored in `IndexedDB` as a non-extractable `CryptoKey`.
//!
//! Struct holds **no** `!Send` JS state (only `Mutex<HashMap>` + config), so it
//! stays `Send + Sync` and satisfies the trait unchanged; all JS/async work runs
//! in `open`/`spawn_local`/`persist_all`. `WebCrypto` is reached via
//! `js_sys::global()` so it works from both the main thread (`Window`) and a
//! **Web Worker** (`WorkerGlobalScope`) — `IndexedDB` already resolves the same
//! way through the `idb` crate, so the whole store is Worker-capable (ADR-0015).

use std::collections::HashMap;
use std::sync::Mutex;

use idb::builder::ObjectStoreBuilder;
use idb::{Database, TransactionMode};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{AesGcmParams, AesKeyGenParams, CryptoKey, SubtleCrypto, Window, WorkerGlobalScope};

use crate::error::KupferdrahtError;
use crate::keystore::SecureKeyStore;

const KEYSTORE_DB: &str = "kupferdraht-keystore";
const META: &str = "meta";
const SECRETS: &str = "secrets";
const WRAP_KEY_ID: &str = "wrap-key";

fn js_err(ctx: &str, e: &JsValue) -> KupferdrahtError {
    KupferdrahtError::Generic(format!("keystore {ctx}: {e:?}"))
}

fn idb_err(ctx: &str, e: &idb::Error) -> KupferdrahtError {
    KupferdrahtError::Generic(format!("keystore {ctx}: {e}"))
}

/// The `WebCrypto` `SubtleCrypto` handle, from whichever global scope Kupferdraht
/// runs in — `Window` (main thread) or `WorkerGlobalScope` (Web Worker). All
/// `IndexedDB` access already resolves via `js_sys::global()` (the `idb` crate),
/// so this was the last main-thread-only dependency.
fn subtle() -> Result<SubtleCrypto, KupferdrahtError> {
    let global = js_sys::global();
    let crypto = if let Some(win) = global.dyn_ref::<Window>() {
        win.crypto()
    } else if let Some(worker) = global.dyn_ref::<WorkerGlobalScope>() {
        worker.crypto()
    } else {
        return Err(KupferdrahtError::Generic(
            "keystore: global scope is neither Window nor WorkerGlobalScope".into(),
        ));
    }
    .map_err(|e| js_err("crypto", &e))?;
    Ok(crypto.subtle())
}

async fn open_db() -> Result<Database, KupferdrahtError> {
    Database::builder(KEYSTORE_DB)
        .version(1)
        .add_object_store(ObjectStoreBuilder::new(META))
        .add_object_store(ObjectStoreBuilder::new(SECRETS))
        .build()
        .await
        .map_err(|e| idb_err("open db", &e))
}

/// Load the non-extractable AES-GCM wrap key, generating + persisting it on
/// first use.
async fn wrap_key(db: &Database) -> Result<CryptoKey, KupferdrahtError> {
    let tx = db
        .transaction(&[META], TransactionMode::ReadOnly)
        .map_err(|e| idb_err("tx", &e))?;
    let os = tx.object_store(META).map_err(|e| idb_err("store", &e))?;
    let found = os
        .get(JsValue::from_str(WRAP_KEY_ID))
        .map_err(|e| idb_err("get", &e))?
        .await
        .map_err(|e| idb_err("get", &e))?;
    if let Some(v) = found {
        return v.dyn_into::<CryptoKey>().map_err(|_| {
            KupferdrahtError::Generic("keystore: stored wrap key is not a CryptoKey".into())
        });
    }

    let algo = AesKeyGenParams::new("AES-GCM", 256);
    let usages = js_sys::Array::of2(&JsValue::from_str("encrypt"), &JsValue::from_str("decrypt"));
    let promise = subtle()?
        .generate_key_with_object(&algo, false, &usages)
        .map_err(|e| js_err("generateKey", &e))?;
    let key: CryptoKey = JsFuture::from(promise)
        .await
        .map_err(|e| js_err("generateKey", &e))?
        .dyn_into()
        .map_err(|_| {
            KupferdrahtError::Generic("keystore: generateKey did not return a CryptoKey".into())
        })?;

    let tx = db
        .transaction(&[META], TransactionMode::ReadWrite)
        .map_err(|e| idb_err("tx", &e))?;
    let os = tx.object_store(META).map_err(|e| idb_err("store", &e))?;
    os.put(&key, Some(&JsValue::from_str(WRAP_KEY_ID)))
        .map_err(|e| idb_err("put", &e))?
        .await
        .map_err(|e| idb_err("put", &e))?;
    tx.commit()
        .map_err(|e| idb_err("commit", &e))?
        .await
        .map_err(|e| idb_err("commit", &e))?;
    Ok(key)
}

#[derive(Serialize, Deserialize)]
struct WrappedSecret {
    iv: Vec<u8>,
    ct: Vec<u8>,
}

async fn encrypt(key: &CryptoKey, plaintext: &[u8]) -> Result<WrappedSecret, KupferdrahtError> {
    let mut iv = [0u8; 12];
    OsRng.fill_bytes(&mut iv);
    let iv_array = js_sys::Uint8Array::from(&iv[..]);
    let algo = AesGcmParams::new_with_u8_array("AES-GCM", &iv_array);
    let data = js_sys::Uint8Array::from(plaintext);
    let promise = subtle()?
        .encrypt_with_object_and_js_u8_array(&algo, key, &data)
        .map_err(|e| js_err("encrypt", &e))?;
    let buf = JsFuture::from(promise)
        .await
        .map_err(|e| js_err("encrypt", &e))?;
    Ok(WrappedSecret {
        iv: iv.to_vec(),
        ct: js_sys::Uint8Array::new(&buf).to_vec(),
    })
}

async fn decrypt(key: &CryptoKey, ws: &WrappedSecret) -> Result<Vec<u8>, KupferdrahtError> {
    let iv_array = js_sys::Uint8Array::from(&ws.iv[..]);
    let algo = AesGcmParams::new_with_u8_array("AES-GCM", &iv_array);
    let data = js_sys::Uint8Array::from(&ws.ct[..]);
    let promise = subtle()?
        .decrypt_with_object_and_js_u8_array(&algo, key, &data)
        .map_err(|e| js_err("decrypt", &e))?;
    let buf = JsFuture::from(promise)
        .await
        .map_err(|e| js_err("decrypt", &e))?;
    Ok(js_sys::Uint8Array::new(&buf).to_vec())
}

/// Wrap + upsert one secret in `IndexedDB`.
async fn persist_one(id: &str, secret: &[u8]) -> Result<(), KupferdrahtError> {
    let db = open_db().await?;
    let key = wrap_key(&db).await?;
    let ws = encrypt(&key, secret).await?;
    let value = serde_wasm_bindgen::to_value(&ws)
        .map_err(|e| KupferdrahtError::Generic(format!("keystore serialize: {e}")))?;
    let tx = db
        .transaction(&[SECRETS], TransactionMode::ReadWrite)
        .map_err(|e| idb_err("tx", &e))?;
    let os = tx.object_store(SECRETS).map_err(|e| idb_err("store", &e))?;
    os.put(&value, Some(&JsValue::from_str(id)))
        .map_err(|e| idb_err("put", &e))?
        .await
        .map_err(|e| idb_err("put", &e))?;
    tx.commit()
        .map_err(|e| idb_err("commit", &e))?
        .await
        .map_err(|e| idb_err("commit", &e))?;
    Ok(())
}

async fn delete_one(id: &str) -> Result<(), KupferdrahtError> {
    let db = open_db().await?;
    let tx = db
        .transaction(&[SECRETS], TransactionMode::ReadWrite)
        .map_err(|e| idb_err("tx", &e))?;
    let os = tx.object_store(SECRETS).map_err(|e| idb_err("store", &e))?;
    os.delete(JsValue::from_str(id))
        .map_err(|e| idb_err("delete", &e))?
        .await
        .map_err(|e| idb_err("delete", &e))?;
    tx.commit()
        .map_err(|e| idb_err("commit", &e))?
        .await
        .map_err(|e| idb_err("commit", &e))?;
    Ok(())
}

/// A persistent [`SecureKeyStore`] over `IndexedDB` + `WebCrypto`.
pub struct IndexedDbKeyStore {
    secrets: Mutex<HashMap<String, Vec<u8>>>,
}

impl IndexedDbKeyStore {
    /// Open the keystore, preloading + decrypting every stored secret. Call once
    /// at startup (async), then use it as an `Arc<dyn SecureKeyStore>`.
    ///
    /// # Errors
    /// `KupferdrahtError::Generic` on `IndexedDB` / `WebCrypto` failure.
    pub async fn open() -> Result<Self, KupferdrahtError> {
        let db = open_db().await?;
        let key = wrap_key(&db).await?;
        let tx = db
            .transaction(&[SECRETS], TransactionMode::ReadOnly)
            .map_err(|e| idb_err("tx", &e))?;
        let os = tx.object_store(SECRETS).map_err(|e| idb_err("store", &e))?;
        let ids = os
            .get_all_keys(None, None)
            .map_err(|e| idb_err("keys", &e))?
            .await
            .map_err(|e| idb_err("keys", &e))?;
        let values = os
            .get_all(None, None)
            .map_err(|e| idb_err("get_all", &e))?
            .await
            .map_err(|e| idb_err("get_all", &e))?;
        let mut map = HashMap::with_capacity(ids.len());
        for (id_js, value) in ids.into_iter().zip(values) {
            let Some(id) = id_js.as_string() else {
                continue;
            };
            let ws: WrappedSecret = serde_wasm_bindgen::from_value(value)
                .map_err(|e| KupferdrahtError::Generic(format!("keystore deserialize: {e}")))?;
            map.insert(id, decrypt(&key, &ws).await?);
        }
        Ok(Self {
            secrets: Mutex::new(map),
        })
    }

    /// Await a guaranteed persist of every in-memory secret (durability flush).
    /// The sync `set_secret` writes best-effort in the background; a consumer
    /// that must be sure a freshly-generated key survives a reload calls this.
    ///
    /// # Errors
    /// `KupferdrahtError::Generic` on `IndexedDB` / `WebCrypto` failure.
    pub async fn persist_all(&self) -> Result<(), KupferdrahtError> {
        let entries: Vec<(String, Vec<u8>)> = self
            .secrets
            .lock()
            .map_err(|e| KupferdrahtError::Generic(format!("keystore lock: {e}")))?
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (id, secret) in entries {
            persist_one(&id, &secret).await?;
        }
        Ok(())
    }
}

impl SecureKeyStore for IndexedDbKeyStore {
    fn get_secret(&self, id: String) -> Result<Option<Vec<u8>>, KupferdrahtError> {
        Ok(self
            .secrets
            .lock()
            .map_err(|e| KupferdrahtError::Generic(format!("keystore lock: {e}")))?
            .get(&id)
            .cloned())
    }

    fn set_secret(&self, id: String, secret: Vec<u8>) -> Result<(), KupferdrahtError> {
        self.secrets
            .lock()
            .map_err(|e| KupferdrahtError::Generic(format!("keystore lock: {e}")))?
            .insert(id.clone(), secret.clone());
        // Background persist, tracked (Step 104): the key-creating paths
        // await `flush_pending_writes` before they use a new key.
        let done = super::pending_writes::track_write();
        wasm_bindgen_futures::spawn_local(async move {
            let result = persist_one(&id, &secret).await;
            if let Err(e) = &result {
                tracing::error!(%id, error = %e, "keystore: persisting a secret failed");
            }
            let _ = done.send(result);
        });
        Ok(())
    }

    fn delete_secret(&self, id: String) -> Result<(), KupferdrahtError> {
        self.secrets
            .lock()
            .map_err(|e| KupferdrahtError::Generic(format!("keystore lock: {e}")))?
            .remove(&id);
        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = delete_one(&id).await {
                tracing::error!(%id, error = %e, "keystore: deleting a secret failed");
            }
        });
        Ok(())
    }
}
