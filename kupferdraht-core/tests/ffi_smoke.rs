// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 20a: smoke-test the UniFFI surface from the consumer side.
//!
//! Exercises the exported free functions the same way the generated
//! Kotlin/Swift bindings do — proving the proc-macro export path is
//! wired for the thin Client/login/keystore slice.

use std::sync::Arc;

use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::{KupferdrahtError, ServerConfig};

#[test]
fn kupferdraht_version_reports_crate_version() {
    assert_eq!(
        kupferdraht::ffi::kupferdraht_version(),
        env!("CARGO_PKG_VERSION")
    );
}

#[test]
fn parse_user_id_returns_structured_parts() {
    // The exported `parse_user_id` validates a flat id string and hands
    // foreign callers the structured `QualifiedUserId` — `localpart` and
    // `domain` as separate fields, never a flattened string.
    let id = kupferdraht::ffi::parse_user_id("alice@example.test").expect("well-formed id");
    assert_eq!(id.localpart, "alice");
    assert_eq!(id.domain, "example.test");
}

#[test]
fn parse_user_id_rejects_a_malformed_id() {
    // Validation runs in Rust (`UserId::parse`), so a missing domain
    // surfaces as a typed error across the FFI boundary, not a
    // silently-malformed record.
    let result = kupferdraht::ffi::parse_user_id("no-domain");
    assert!(
        matches!(result, Err(KupferdrahtError::InvalidInput(_))),
        "an id without a domain must reject, got {result:?}"
    );
}

#[test]
fn ffi_timeline_tracks_core_parity_surface() {
    // Public-API ↔ FFI parity (CLAUDE.md): the core Timeline's
    // `room_id` / `send_text_auto_join` must exist on the FFI wrapper.
    // Compile-time pins only — a runtime call needs a logged-in client,
    // which the https-only FFI manager cannot provide offline.
    let _room_id: fn(&kupferdraht::ffi::Timeline) -> String = kupferdraht::ffi::Timeline::room_id;
    let _auto_join = kupferdraht::ffi::Timeline::send_text_auto_join;
    // Step 26: the Knock / Location send methods must exist on the FFI wrapper.
    let _send_knock = kupferdraht::ffi::Timeline::send_knock;
    let _send_location = kupferdraht::ffi::Timeline::send_location;
    // Step 86: the self-room lookup (sync, local cache) must exist on
    // the FFI `Client` next to `dm_room`.
    let _self_room: fn(&kupferdraht::ffi::Client) -> Option<Arc<kupferdraht::ffi::Room>> =
        kupferdraht::ffi::Client::self_room;
    // Step 79: failed-send recovery — per-message + per-room resend on
    // the FFI `Timeline`, the failed-row enumeration on the FFI `Room`.
    let _resend = kupferdraht::ffi::Timeline::resend;
    let _resend_failed = kupferdraht::ffi::Timeline::resend_failed;
    let _failed_sends = kupferdraht::ffi::Room::failed_sends;
}

#[tokio::test]
async fn manager_register_rejects_plain_http_before_any_network() {
    // `register` is a method on the FFI manager (no free function, no
    // caller-supplied keystore/base_dir) and shares the manager's
    // https-only stance: the transport guard fires synchronously,
    // before any network I/O.
    let dir = tempfile::tempdir().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let kupferdraht =
        kupferdraht::ffi::Kupferdraht::open(dir.path().to_string_lossy().into_owned(), keystore)
            .expect("open manager");
    let server = ServerConfig {
        api_base_url: "http://insecure.example".to_owned(),
        websocket_url: "ws://insecure.example".to_owned(),
        title: "smoke".to_owned(),
    };

    let result = kupferdraht
        .register(
            server,
            "Alice".to_owned(),
            "alice@example.test".to_owned(),
            "hunter2hunter2".to_owned(),
        )
        .await;

    assert!(
        matches!(result, Err(KupferdrahtError::InvalidInput(_))),
        "plain-http server url must reject via the https guard"
    );
}

#[tokio::test]
async fn manager_login_rejects_plain_http_before_any_network() {
    // The FFI `Kupferdraht` manager has no `permit_http` knob, so it always
    // enforces HTTPS. A plain-`http://` server URL must surface as
    // `InvalidInput` from the transport's https guard — which fires
    // synchronously, before any network I/O.
    let dir = tempfile::tempdir().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let kupferdraht =
        kupferdraht::ffi::Kupferdraht::open(dir.path().to_string_lossy().into_owned(), keystore)
            .expect("open manager");
    let server = ServerConfig {
        api_base_url: "http://insecure.example".to_owned(),
        websocket_url: "ws://insecure.example".to_owned(),
        title: "smoke".to_owned(),
    };

    let result = kupferdraht
        .login(
            server,
            "alice@example.test".to_owned(),
            "pw".to_owned(),
            None,
            None,
        )
        .await;

    assert!(
        matches!(result, Err(KupferdrahtError::InvalidInput(_))),
        "plain-http server url must reject via the https guard"
    );
}
