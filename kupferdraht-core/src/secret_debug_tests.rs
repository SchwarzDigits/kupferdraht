// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 101 — `Debug` never prints a secret (review finding H6). TDD red.
//!
//! Every type that carries a token, a password, a join or activation
//! code or an attachment key is built with a sentinel in each secret field;
//! its `Debug` output — what a `?value` log field or an `{:?}` in an
//! error would print — must not contain it.

use std::sync::Weak;

use crate::encryption::attachment_crypto::encrypt_reader;
use crate::storage::{AttachmentRecord, SessionRecord};
use crate::timeline::AttachmentSummary;
use crate::wire::dto::{
    ActivateRequest, ConversationCodeInfo, CreateConversationCodeRequest, JoinConversationByCode,
    LoginOutcome, LoginResponse, NewUserRequest,
};
use crate::wire::http::assets::AssetUploadResponse;
use crate::wire::http::auth::RebindOutcome;
use crate::wire::proto::AssetMessageInput;

const SECRET: &str = "SECRET-101";

fn assert_hidden(label: &str, debug: &str) {
    assert!(
        !debug.contains(SECRET),
        "{label}'s Debug prints a secret: {debug}"
    );
}

#[test]
fn session_and_login_tokens_are_hidden() {
    let session = SessionRecord {
        user_id: "user@example.test".into(),
        device_id: "device".into(),
        access_token: format!("{SECRET}-access"),
        refresh_token: format!("{SECRET}-zuid"),
        cookie_label: None,
        api_version: 14,
        access_token_issued_at_ms: 1,
    };
    assert_hidden("SessionRecord", &format!("{session:?}"));
    let response = LoginResponse {
        access_token: format!("{SECRET}-access"),
        token_type: "Bearer".into(),
        expires_in: 900,
        user: "user".into(),
    };
    assert_hidden("LoginResponse", &format!("{response:?}"));
    let outcome = LoginOutcome {
        access_token: format!("{SECRET}-access"),
        user: "user".into(),
        refresh_token: format!("{SECRET}-zuid"),
    };
    assert_hidden("LoginOutcome", &format!("{outcome:?}"));
    let rebind = RebindOutcome {
        access_token: format!("{SECRET}-access"),
        refresh_token: format!("{SECRET}-zuid"),
    };
    assert_hidden("RebindOutcome", &format!("{rebind:?}"));
}

#[test]
fn attachment_keys_and_tokens_are_hidden() {
    let attachment = AttachmentRecord {
        key_ct: SECRET.as_bytes().to_vec(),
        attachment_token: Some(format!("{SECRET}-token")),
        ..AttachmentRecord::default()
    };
    // Bytes print as numbers in a derived Debug — check those too.
    let key_bytes = format!("{:?}", SECRET.as_bytes());
    let debug = format!("{attachment:?}");
    assert_hidden("AttachmentRecord", &debug);
    assert!(
        !debug.contains(&key_bytes[1..key_bytes.len() - 1]),
        "AttachmentRecord prints the key bytes: {debug}"
    );
    let summary = AttachmentSummary {
        message_id: "m".into(),
        mime_type: "image/png".into(),
        filename: None,
        size_bytes: 1,
        sender_user_id: "u@example.test".into(),
        room_id: "r@example.test".into(),
        attachment_id: "a".into(),
        attachment_token: Some(format!("{SECRET}-token")),
        attachment_domain: "example.test".into(),
        cipher: "AES_CBC".into(),
        cache_path: None,
        sha256: Vec::new(),
        key_ct: SECRET.as_bytes().to_vec(),
        client: Weak::new(),
    };
    let debug = format!("{summary:?}");
    assert_hidden("AttachmentSummary", &debug);
    assert!(
        !debug.contains(&key_bytes[1..key_bytes.len() - 1]),
        "AttachmentSummary prints the key bytes: {debug}"
    );
    let meta = crate::ffi::AttachmentMeta::from(&summary);
    assert_hidden("AttachmentMeta", &format!("{meta:?}"));
    let input = AssetMessageInput {
        mime_type: "image/png",
        size: 1,
        filename: None,
        otr_key: SECRET.as_bytes(),
        sha256: &[],
        asset_id: "a",
        asset_token: Some("SECRET-101-token"),
        asset_domain: "example.test",
    };
    let debug = format!("{input:?}");
    assert_hidden("AssetMessageInput", &debug);
    assert!(
        !debug.contains(&key_bytes[1..key_bytes.len() - 1]),
        "AssetMessageInput prints the key bytes: {debug}"
    );
    let upload = AssetUploadResponse {
        key: "3-1-attachment".into(),
        token: Some(format!("{SECRET}-token")),
        domain: None,
        expires: None,
    };
    assert_hidden("AssetUploadResponse", &format!("{upload:?}"));
}

/// The send-side attachment key lives in a `Zeroizing<[u8; 32]>` whose own
/// derived `Debug` prints the bytes — the holder must redact it.
#[test]
fn send_side_attachment_key_is_hidden() {
    let dir = tempfile::tempdir().expect("tempdir");
    let encrypted = encrypt_reader(&b"asset plaintext"[..], dir.path()).expect("encrypt");
    let key_bytes = format!("{:?}", &encrypted.key_material()[..]);
    let debug = format!("{encrypted:?}");
    assert!(
        !debug.contains(&key_bytes[1..key_bytes.len() - 1]),
        "EncryptedAttachment prints the key bytes: {debug}"
    );
    assert!(
        debug.contains("<32 bytes>"),
        "key redacted to its length: {debug}"
    );
}

#[test]
fn passwords_and_codes_are_hidden() {
    let create = CreateConversationCodeRequest {
        password: Some("SECRET-101-pw"),
    };
    assert_hidden("CreateConversationCodeRequest", &format!("{create:?}"));
    let info = ConversationCodeInfo {
        key: format!("{SECRET}-key"),
        code: format!("{SECRET}-code"),
        uri: format!("https://example.test/join?key={SECRET}"),
        has_password: false,
    };
    assert_hidden("ConversationCodeInfo", &format!("{info:?}"));
    let join = JoinConversationByCode {
        key: "SECRET-101-key",
        code: "SECRET-101-code",
        password: Some("SECRET-101-pw"),
    };
    assert_hidden("JoinConversationByCode", &format!("{join:?}"));
    let user = NewUserRequest {
        name: "Alice".into(),
        password: Some(format!("{SECRET}-pw")),
        ..NewUserRequest::default()
    };
    assert_hidden("NewUserRequest", &format!("{user:?}"));
    let activate = ActivateRequest {
        code: "SECRET-101-code",
        dryrun: false,
        email: None,
        key: Some("SECRET-101-key"),
        phone: None,
    };
    assert_hidden("ActivateRequest", &format!("{activate:?}"));
}
