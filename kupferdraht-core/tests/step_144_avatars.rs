// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![cfg(not(target_arch = "wasm32"))]
//! Step 144 — avatars: show every user's and the team's, set the own (TDD red).
//!
//! The keys come from `/self`, the stored member list and, for a user in no
//! room, one `list-users`. A download is a public asset behind the usual `302`;
//! what reaches the caller is an image Kupferdraht wrote
//! (`step_144_image_rewrite`), held in a small cache per client.

use std::path::Path;
use std::sync::Arc;

use httpmock::Method::{GET, POST, PUT};
use httpmock::{Mock, MockServer};
use kupferdraht::keystore::{InMemoryKeyStore, SecureKeyStore};
use kupferdraht::storage::RoomMemberRecord;
use kupferdraht::{AvatarSize, Client, Kupferdraht, KupferdrahtError, UserId};
use tempfile::TempDir;

mod common;

#[path = "common/images.rs"]
mod images;

const QUALIFIED: &str = "user-uuid@example.test";
const DOMAIN: &str = "example.test";
const TEAM: &str = "6ba7b810-9dad-41d1-80b4-00c04fd430c8";
const ROOM: &str = "11111111-1111-4111-8111-111111111111";
const PEER: &str = "22222222-2222-4222-8222-222222222222";
const STRANGER: &str = "33333333-3333-4333-8333-333333333333";
const PREVIEW_KEY: &str = "3-1-preview";
const COMPLETE_KEY: &str = "3-1-complete";
const ICON_KEY: &str = "3-1-icon";

fn assets_json(preview: Option<&str>, complete: Option<&str>) -> String {
    let mut assets = Vec::new();
    if let Some(key) = preview {
        assets.push(format!(
            r#"{{"key":"{key}","size":"preview","type":"image"}}"#
        ));
    }
    if let Some(key) = complete {
        assets.push(format!(
            r#"{{"key":"{key}","size":"complete","type":"image"}}"#
        ));
    }
    format!("[{}]", assets.join(","))
}

fn self_body(team: Option<&str>, assets: &str) -> String {
    let team = team.map_or(String::new(), |t| format!(r#","team":"{t}""#));
    format!(
        r#"{{"id":"user-uuid","qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}},"handle":"alice","name":"Alice Example","email":"alice@example.test","assets":{assets}{team}}}"#
    )
}

fn user_json(id: &str, assets: &str) -> String {
    format!(
        r#"{{"qualified_id":{{"id":"{id}","domain":"{DOMAIN}"}},"name":"Someone","handle":"someone","assets":{assets}}}"#
    )
}

fn team_body(icon: &str) -> String {
    format!(
        r#"{{"binding":true,"creator":"creator-uuid","icon":"{icon}","id":"{TEAM}","name":"Fixture Team","splash_screen":"default"}}"#
    )
}

/// `GET /v14/assets/{domain}/{key}` answering with the image itself.
fn mount_asset<'a>(server: &'a MockServer, key: &str, body: &[u8]) -> Mock<'a> {
    let path = format!("/v14/assets/{DOMAIN}/{key}");
    let body = body.to_vec();
    server.mock(move |when, then| {
        when.method(GET)
            .path(path)
            .header("authorization", "Bearer rebound.jwt.token");
        then.status(200)
            .header("content-type", "application/octet-stream")
            .body(body);
    })
}

/// Any download, to count that none was made.
fn mount_any_asset(server: &MockServer) -> Mock<'_> {
    server.mock(|when, then| {
        when.path_includes("/assets/");
        then.status(500);
    })
}

struct Session {
    _home: TempDir,
    app: Kupferdraht,
    client: Arc<Client>,
}

async fn login_with_self(server: &MockServer, self_json: &str) -> Session {
    common::login_mocks::mount_login_with_self(server, self_json);
    common::login_mocks::mount_device_registration(server);
    let home = TempDir::new().expect("tempdir");
    let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
    let app = Kupferdraht::open(home.path().to_path_buf(), keystore)
        .expect("open kupferdraht")
        .permit_http();
    let client = app
        .login(
            common::server_config(&server.base_url()),
            "alice@example.test".into(),
            "pw".into(),
            None,
            None,
        )
        .await
        .expect("login")
        .register_device()
        .await
        .expect("register device");
    Session {
        _home: home,
        app,
        client,
    }
}

async fn login(server: &MockServer, assets: &str) -> Session {
    login_with_self(server, &self_body(None, assets)).await
}

async fn login_in_team(server: &MockServer, icon: &str) -> Session {
    server.mock(|when, then| {
        when.method(GET).path(format!("/v14/teams/{TEAM}"));
        then.status(200)
            .header("content-type", "application/json")
            .body(team_body(icon));
    });
    login_with_self(server, &self_body(Some(TEAM), "[]")).await
}

/// One room with one other member, whose profile names `assets`.
fn mount_room_with_peer(server: &MockServer, assets: &str) {
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list-ids");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"qualified_conversations":[{{"id":"{ROOM}","domain":"{DOMAIN}"}}],"has_more":false,"paging_state":"END"}}"#
            ));
    });
    server.mock(|when, then| {
        when.method(POST).path("/v14/conversations/list");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(
                r#"{{"found":[{{"qualified_id":{{"id":"{ROOM}","domain":"{DOMAIN}"}},"type":0,"protocol":"mls","epoch":0,"group_id":"Z3JwLWFh","name":"Room","members":{{"self":{{"qualified_id":{{"id":"user-uuid","domain":"{DOMAIN}"}}}},"others":[{{"qualified_id":{{"id":"{PEER}","domain":"{DOMAIN}"}}}}]}}}}],"not_found":[],"failed":[]}}"#
            ));
    });
    let found = user_json(PEER, assets);
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(r#"{{"found":[{found}]}}"#));
    });
}

fn peer() -> UserId {
    UserId::parse(&format!("{PEER}@{DOMAIN}")).expect("peer id")
}

fn own() -> UserId {
    UserId::parse(QUALIFIED).expect("own id")
}

fn stranger() -> UserId {
    UserId::parse(&format!("{STRANGER}@{DOMAIN}")).expect("stranger id")
}

// --- reading ---------------------------------------------------------------

#[tokio::test]
async fn a_member_s_avatar_comes_from_the_stored_keys_written_anew() {
    let server = MockServer::start();
    let session = login(&server, "[]").await;
    mount_room_with_peer(&server, &assets_json(Some(PREVIEW_KEY), Some(COMPLETE_KEY)));
    session.client.sync_metadata().await.expect("sync metadata");
    let input = images::flat_jpeg(64, 64);
    let preview = mount_asset(&server, PREVIEW_KEY, &input);
    let complete = mount_asset(&server, COMPLETE_KEY, &images::flat_jpeg(200, 200));

    let thumbnail = session
        .client
        .avatar(&peer(), AvatarSize::Thumbnail)
        .await
        .expect("thumbnail");
    let full = session
        .client
        .avatar(&peer(), AvatarSize::Full)
        .await
        .expect("full");

    let thumbnail = thumbnail.expect("the member has an avatar");
    assert_ne!(thumbnail.bytes, input, "written anew");
    assert_eq!(thumbnail.mime_type, "image/jpeg");
    assert_eq!(images::read_jpeg(&full.expect("full").bytes).0, 200);
    preview.assert_calls(1);
    complete.assert_calls(1);
}

#[tokio::test]
async fn avatar_key_and_the_member_entry_carry_the_version_marker() {
    let server = MockServer::start();
    let session = login(&server, "[]").await;
    mount_room_with_peer(&server, &assets_json(Some(PREVIEW_KEY), Some(COMPLETE_KEY)));
    session.client.sync_metadata().await.expect("sync metadata");

    let key = session
        .client
        .avatar_key(&peer())
        .await
        .expect("avatar_key");

    assert_eq!(key.as_deref(), Some(PREVIEW_KEY), "the thumbnail's key");
    let room = session
        .client
        .room(&format!("{ROOM}@{DOMAIN}"))
        .await
        .expect("room");
    let member = room
        .members()
        .await
        .expect("members")
        .into_iter()
        .find(|m| !m.is_self)
        .expect("the other member");
    assert_eq!(member.avatar_key(), Some(PREVIEW_KEY));
}

/// A profile with only the complete image: its key is the marker, and a
/// thumbnail is made from it.
#[tokio::test]
async fn one_size_alone_serves_both() {
    let server = MockServer::start();
    let session = login(&server, "[]").await;
    mount_room_with_peer(&server, &assets_json(None, Some(COMPLETE_KEY)));
    session.client.sync_metadata().await.expect("sync metadata");
    let complete = mount_asset(&server, COMPLETE_KEY, &images::flat_jpeg(600, 600));

    assert_eq!(
        session
            .client
            .avatar_key(&peer())
            .await
            .expect("avatar_key")
            .as_deref(),
        Some(COMPLETE_KEY)
    );
    let thumbnail = session
        .client
        .avatar(&peer(), AvatarSize::Thumbnail)
        .await
        .expect("thumbnail")
        .expect("made from the complete image");

    assert_eq!(images::read_jpeg(&thumbnail.bytes).0, 280);
    complete.assert_calls(1);
}

/// Wire's Android client names one key for both sizes.
#[tokio::test]
async fn one_key_named_for_both_sizes_serves_both() {
    let server = MockServer::start();
    let session = login(&server, "[]").await;
    mount_room_with_peer(&server, &assets_json(Some(PREVIEW_KEY), Some(PREVIEW_KEY)));
    session.client.sync_metadata().await.expect("sync metadata");
    mount_asset(&server, PREVIEW_KEY, &images::flat_jpeg(280, 280));

    for size in [AvatarSize::Thumbnail, AvatarSize::Full] {
        let avatar = session.client.avatar(&peer(), size).await.expect("avatar");
        assert_eq!(images::read_jpeg(&avatar.expect("avatar").bytes).0, 280);
    }
}

#[tokio::test]
async fn a_member_without_assets_has_no_avatar_and_no_download() {
    let server = MockServer::start();
    let session = login(&server, "[]").await;
    mount_room_with_peer(&server, "[]");
    session.client.sync_metadata().await.expect("sync metadata");
    let any_asset = mount_any_asset(&server);

    let avatar = session
        .client
        .avatar(&peer(), AvatarSize::Thumbnail)
        .await
        .expect("avatar");

    assert!(avatar.is_none());
    assert_eq!(session.client.avatar_key(&peer()).await.expect("key"), None);
    any_asset.assert_calls(0);
}

/// An asset of another type or an unknown size is no avatar, and does not fail
/// the profile that names it.
#[tokio::test]
async fn an_unknown_asset_is_skipped() {
    let server = MockServer::start();
    let session = login(&server, "[]").await;
    mount_room_with_peer(
        &server,
        r#"[{"key":"3-1-video","size":"preview","type":"video"},{"key":"3-1-huge","size":"huge","type":"image"},{"key":"3-1-good","size":"complete","type":"image"}]"#,
    );
    session.client.sync_metadata().await.expect("sync metadata");

    let key = session
        .client
        .avatar_key(&peer())
        .await
        .expect("avatar_key");

    assert_eq!(key.as_deref(), Some("3-1-good"));
}

#[tokio::test]
async fn the_own_avatar_comes_from_self() {
    let server = MockServer::start();
    let session = login(&server, &assets_json(Some(PREVIEW_KEY), Some(COMPLETE_KEY))).await;
    mount_asset(&server, PREVIEW_KEY, &images::flat_jpeg(64, 64));

    assert_eq!(
        session
            .client
            .avatar_key(&own())
            .await
            .expect("key")
            .as_deref(),
        Some(PREVIEW_KEY)
    );
    assert!(
        session
            .client
            .avatar(&own(), AvatarSize::Thumbnail)
            .await
            .expect("avatar")
            .is_some()
    );
}

#[tokio::test]
async fn a_user_in_no_room_is_looked_up_once() {
    let server = MockServer::start();
    let session = login(&server, "[]").await;
    let found = user_json(STRANGER, &assets_json(Some(PREVIEW_KEY), None));
    let lookup = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/list-users")
            .body_includes(STRANGER);
        then.status(200)
            .header("content-type", "application/json")
            .body(format!(r#"{{"found":[{found}]}}"#));
    });
    mount_asset(&server, PREVIEW_KEY, &images::flat_jpeg(64, 64));

    let avatar = session
        .client
        .avatar(&stranger(), AvatarSize::Thumbnail)
        .await
        .expect("avatar");

    assert!(avatar.is_some());
    lookup.assert_calls(1);
}

#[tokio::test]
async fn a_user_the_backend_does_not_know_has_no_avatar() {
    let server = MockServer::start();
    let session = login(&server, "[]").await;
    server.mock(|when, then| {
        when.method(POST).path("/v14/list-users");
        then.status(200)
            .header("content-type", "application/json")
            .body(r#"{"found":[]}"#);
    });
    let any_asset = mount_any_asset(&server);

    let avatar = session
        .client
        .avatar(&stranger(), AvatarSize::Thumbnail)
        .await
        .expect("avatar");

    assert!(avatar.is_none());
    any_asset.assert_calls(0);
}

/// The download follows the `302` to the asset store by hand, without the
/// bearer token, as for a file (Step 141). The signature never reaches an error.
#[tokio::test]
async fn the_redirect_is_followed_without_credentials() {
    let server = MockServer::start();
    let asset_store = MockServer::start();
    let session = login(&server, &assets_json(Some(PREVIEW_KEY), None)).await;
    let location = asset_store.url("/assets/fixture?X-Amz-Signature=SECRET-SIGNATURE");
    server.mock(|when, then| {
        when.method(GET)
            .path(format!("/v14/assets/{DOMAIN}/{PREVIEW_KEY}"));
        then.status(302).header("location", location);
    });
    let image = images::flat_jpeg(64, 64);
    let hop = asset_store.mock(|when, then| {
        when.method(GET)
            .path("/assets/fixture")
            .header_missing("authorization");
        then.status(200).body(image);
    });

    let avatar = session
        .client
        .avatar(&own(), AvatarSize::Thumbnail)
        .await
        .expect("avatar");

    assert!(avatar.is_some());
    hop.assert_calls(1);
}

#[tokio::test]
async fn an_answer_over_the_cap_fails() {
    let server = MockServer::start();
    let session = login(&server, &assets_json(Some(PREVIEW_KEY), None)).await;
    let mut body = images::flat_jpeg(8, 8);
    body.resize(kupferdraht::avatar::AVATAR_MAX_INPUT_BYTES + 1, 0);
    mount_asset(&server, PREVIEW_KEY, &body);

    let result = session.client.avatar(&own(), AvatarSize::Thumbnail).await;

    assert!(
        matches!(result, Err(KupferdrahtError::BackendRejected { .. })),
        "{result:?}"
    );
}

/// What the store holds under an avatar's key is someone else's choice.
#[tokio::test]
async fn a_download_that_is_no_image_is_no_avatar() {
    let server = MockServer::start();
    let session = login(&server, &assets_json(Some(PREVIEW_KEY), None)).await;
    mount_asset(
        &server,
        PREVIEW_KEY,
        b"<svg><script>alert(1)</script></svg>",
    );

    let avatar = session
        .client
        .avatar(&own(), AvatarSize::Thumbnail)
        .await
        .expect("no error");

    assert!(avatar.is_none());
}

#[tokio::test]
async fn a_key_that_is_no_path_segment_goes_out_nowhere() {
    let server = MockServer::start();
    let session = login(&server, &assets_json(Some("a/b"), None)).await;
    let any_asset = mount_any_asset(&server);

    let result = session.client.avatar(&own(), AvatarSize::Thumbnail).await;

    assert!(!matches!(result, Ok(Some(_))), "{result:?}");
    any_asset.assert_calls(0);
}

// --- the cache -------------------------------------------------------------

#[tokio::test]
async fn the_second_call_is_served_from_the_cache() {
    let server = MockServer::start();
    let session = login(&server, &assets_json(Some(PREVIEW_KEY), None)).await;
    let download = mount_asset(&server, PREVIEW_KEY, &images::flat_jpeg(64, 64));

    let first = session
        .client
        .avatar(&own(), AvatarSize::Thumbnail)
        .await
        .expect("first");
    let second = session
        .client
        .avatar(&own(), AvatarSize::Thumbnail)
        .await
        .expect("second");

    assert_eq!(first, second);
    download.assert_calls(1);
}

#[tokio::test]
async fn an_image_over_a_mebibyte_is_served_but_not_cached() {
    let server = MockServer::start();
    let session = login(&server, &assets_json(None, Some(COMPLETE_KEY))).await;
    let download = mount_asset(&server, COMPLETE_KEY, &images::noise_jpeg(1400, 1400));

    let first = session
        .client
        .avatar(&own(), AvatarSize::Full)
        .await
        .expect("first")
        .expect("image");
    assert!(
        first.bytes.len() > 1024 * 1024,
        "the fixture is over the cap: {}",
        first.bytes.len()
    );
    session
        .client
        .avatar(&own(), AvatarSize::Full)
        .await
        .expect("second");

    download.assert_calls(2);
}

// --- the team's icon -------------------------------------------------------

#[tokio::test]
async fn the_team_icon_is_written_anew() {
    let server = MockServer::start();
    let input = images::flat_jpeg(64, 64);
    let download = mount_asset(&server, ICON_KEY, &input);
    let session = login_in_team(&server, ICON_KEY).await;

    let first = session
        .client
        .team_avatar()
        .await
        .expect("team_avatar")
        .expect("an icon");
    let second = session
        .client
        .team_avatar()
        .await
        .expect("second")
        .expect("an icon");

    assert_ne!(first.bytes, input, "written anew");
    assert_eq!(first, second);
    download.assert_calls(1);
}

#[tokio::test]
async fn a_default_icon_is_none_without_a_download() {
    let server = MockServer::start();
    let any_asset = mount_any_asset(&server);
    let session = login_in_team(&server, "default").await;

    assert_eq!(
        session.client.team_avatar().await.expect("team_avatar"),
        None
    );
    any_asset.assert_calls(0);
}

#[tokio::test]
async fn a_personal_account_has_no_team_icon() {
    let server = MockServer::start();
    let any_asset = mount_any_asset(&server);
    let session = login(&server, "[]").await;

    assert_eq!(
        session.client.team_avatar().await.expect("team_avatar"),
        None
    );
    any_asset.assert_calls(0);
}

#[tokio::test]
async fn a_failed_team_request_leaves_no_icon() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path(format!("/v14/teams/{TEAM}"));
        then.status(500);
    });
    let any_asset = mount_any_asset(&server);
    let session = login_with_self(&server, &self_body(Some(TEAM), "[]")).await;

    assert_eq!(
        session.client.team_avatar().await.expect("team_avatar"),
        None
    );
    any_asset.assert_calls(0);
}

#[tokio::test]
async fn a_resume_keeps_the_team_icon_and_the_own_keys() {
    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path(format!("/v14/teams/{TEAM}"));
        then.status(200)
            .header("content-type", "application/json")
            .body(team_body(ICON_KEY));
    });
    let assets = assets_json(Some(PREVIEW_KEY), Some(COMPLETE_KEY));
    let session = login_with_self(&server, &self_body(Some(TEAM), &assets)).await;
    drop(session.client);
    mount_asset(&server, ICON_KEY, &images::flat_jpeg(64, 64));

    let resumed = session.app.resume(QUALIFIED).await.expect("resume");

    assert!(resumed.team_avatar().await.expect("team_avatar").is_some());
    assert_eq!(
        resumed.avatar_key(&own()).await.expect("key").as_deref(),
        Some(PREVIEW_KEY)
    );
}

// --- setting ---------------------------------------------------------------

/// `POST /v14/assets`, answering `key` to the upload whose body has `marker`.
fn mount_upload<'a>(server: &'a MockServer, marker: &str, key: &str) -> Mock<'a> {
    let marker = marker.to_owned();
    let answer = format!(r#"{{"key":"{key}","domain":"{DOMAIN}"}}"#);
    server.mock(move |when, then| {
        when.method(POST)
            .path("/v14/assets")
            .body_includes(r#""public":true"#)
            .body_includes(r#""retention":"eternal""#)
            .body_includes(r#""filetype":"image/jpeg""#)
            .body_includes(r#""filename":"#)
            .body_includes(marker);
        then.status(201)
            .header("content-type", "application/json")
            .body(answer);
    })
}

#[tokio::test]
async fn upload_avatar_writes_both_sizes_and_names_them_on_the_profile() {
    let server = MockServer::start();
    let session = login(&server, "[]").await;
    let uploads = server.mock(|when, then| {
        when.method(POST)
            .path("/v14/assets")
            .body_includes(r#""public":true"#)
            .body_includes(r#""retention":"eternal""#)
            .body_includes(r#""filetype":"image/jpeg""#);
        then.status(201)
            .header("content-type", "application/json")
            .body(format!(r#"{{"key":"{PREVIEW_KEY}","domain":"{DOMAIN}"}}"#));
    });
    let profile = server.mock(|when, then| {
        when.method(PUT)
            .path("/v14/self")
            .body_includes(r#""assets":["#)
            .body_includes(PREVIEW_KEY);
        then.status(200);
    });

    session
        .client
        .upload_avatar(images::flat_jpeg(600, 600))
        .await
        .expect("upload_avatar");

    uploads.assert_calls(2);
    profile.assert_calls(1);
    assert_eq!(
        session
            .client
            .avatar_key(&own())
            .await
            .expect("key")
            .as_deref(),
        Some(PREVIEW_KEY),
        "answered from the new state at once"
    );
}

/// The two uploads differ in size, and neither is the input.
#[tokio::test]
async fn the_two_uploads_are_the_two_sizes_written_anew() {
    let server = MockServer::start();
    let session = login(&server, "[]").await;
    let input = images::with_raw_exif(&images::flat_jpeg(600, 600), b"MM\0*GPS-POSITION");
    let bodies = Arc::new(std::sync::Mutex::new(Vec::<Vec<u8>>::new()));
    let sink = bodies.clone();
    server.mock(move |when, then| {
        when.method(POST)
            .path("/v14/assets")
            .is_true(move |request| {
                sink.lock().expect("sink").push(request.body_ref().to_vec());
                true
            });
        then.status(201)
            .header("content-type", "application/json")
            .body(format!(r#"{{"key":"{PREVIEW_KEY}","domain":"{DOMAIN}"}}"#));
    });
    server.mock(|when, then| {
        when.method(PUT).path("/v14/self");
        then.status(200);
    });

    session
        .client
        .upload_avatar(input)
        .await
        .expect("upload_avatar");

    let bodies = bodies.lock().expect("bodies");
    let unique: std::collections::BTreeSet<&Vec<u8>> = bodies.iter().collect();
    assert_eq!(unique.len(), 2, "two different uploads");
    for body in unique {
        assert!(
            !body.windows(12).any(|window| window == b"GPS-POSITION"),
            "the metadata stayed behind"
        );
    }
}

#[tokio::test]
async fn upload_avatar_refuses_what_is_no_image_before_any_request() {
    let server = MockServer::start();
    let session = login(&server, "[]").await;
    let uploads = server.mock(|when, then| {
        when.method(POST).path("/v14/assets");
        then.status(201);
    });
    let profile = server.mock(|when, then| {
        when.method(PUT).path("/v14/self");
        then.status(200);
    });
    let mut oversize = images::flat_jpeg(8, 8);
    oversize.resize(kupferdraht::avatar::AVATAR_MAX_INPUT_BYTES + 1, 0);

    for input in [b"<svg></svg>".to_vec(), Vec::new(), oversize] {
        let result = session.client.upload_avatar(input).await;
        assert!(
            matches!(result, Err(KupferdrahtError::InvalidInput(_))),
            "{result:?}"
        );
    }

    uploads.assert_calls(0);
    profile.assert_calls(0);
}

/// A profile the backend refused changes nothing here.
#[tokio::test]
async fn a_refused_profile_update_keeps_the_old_avatar() {
    let server = MockServer::start();
    let session = login(&server, &assets_json(Some(PREVIEW_KEY), None)).await;
    mount_upload(&server, "", COMPLETE_KEY);
    server.mock(|when, then| {
        when.method(PUT).path("/v14/self");
        then.status(403)
            .header("content-type", "application/json")
            .body(r#"{"code":403,"label":"forbidden","message":"no"}"#);
    });

    let result = session
        .client
        .upload_avatar(images::flat_jpeg(64, 64))
        .await;

    assert!(result.is_err());
    assert_eq!(
        session
            .client
            .avatar_key(&own())
            .await
            .expect("key")
            .as_deref(),
        Some(PREVIEW_KEY)
    );
}

#[tokio::test]
async fn a_resume_reads_the_uploaded_keys_from_the_store() {
    let server = MockServer::start();
    let session = login(&server, "[]").await;
    mount_upload(&server, "", COMPLETE_KEY);
    server.mock(|when, then| {
        when.method(PUT).path("/v14/self");
        then.status(200);
    });
    session
        .client
        .upload_avatar(images::flat_jpeg(64, 64))
        .await
        .expect("upload_avatar");
    drop(session.client);

    let resumed = session.app.resume(QUALIFIED).await.expect("resume");

    assert_eq!(
        resumed.avatar_key(&own()).await.expect("key").as_deref(),
        Some(COMPLETE_KEY)
    );
}

#[tokio::test]
async fn remove_avatar_empties_the_profile_and_the_state() {
    let server = MockServer::start();
    let session = login(&server, &assets_json(Some(PREVIEW_KEY), Some(COMPLETE_KEY))).await;
    let profile = server.mock(|when, then| {
        when.method(PUT)
            .path("/v14/self")
            .json_body(serde_json::json!({ "assets": [] }));
        then.status(200);
    });
    let any_asset = mount_any_asset(&server);

    session.client.remove_avatar().await.expect("remove_avatar");

    profile.assert_calls(1);
    assert_eq!(session.client.avatar_key(&own()).await.expect("key"), None);
    assert_eq!(
        session
            .client
            .avatar(&own(), AvatarSize::Thumbnail)
            .await
            .expect("avatar"),
        None
    );
    any_asset.assert_calls(0);
}

/// `remember_self_profile` writes the row whole.
#[tokio::test]
async fn a_profile_write_keeps_the_avatar_keys() {
    let server = MockServer::start();
    let session = login(&server, &assets_json(Some(PREVIEW_KEY), Some(COMPLETE_KEY))).await;
    server.mock(|when, then| {
        when.method(PUT).path("/v14/self");
        then.status(200);
    });

    session
        .client
        .set_display_name("Alice Two")
        .await
        .expect("set_display_name");

    let store = session.client.store().expect("login attached a store");
    let row = store
        .load_self_user(QUALIFIED)
        .await
        .expect("load_self_user")
        .expect("self_user row present");
    assert_eq!(row.avatar_thumbnail_key.as_deref(), Some(PREVIEW_KEY));
    assert_eq!(row.avatar_full_key.as_deref(), Some(COMPLETE_KEY));
}

// --- storage ---------------------------------------------------------------

#[tokio::test]
async fn member_keys_are_stored_and_survive_a_replace_of_the_members() {
    let server = MockServer::start();
    let session = login(&server, "[]").await;
    mount_room_with_peer(&server, &assets_json(Some(PREVIEW_KEY), Some(COMPLETE_KEY)));
    session.client.sync_metadata().await.expect("sync metadata");
    let store = session.client.store().expect("login attached a store");
    let room_id = format!("{ROOM}@{DOMAIN}");
    let peer_id = format!("{PEER}@{DOMAIN}");

    let keys = store
        .member_avatar_keys(&peer_id)
        .await
        .expect("member_avatar_keys")
        .expect("a stored member");
    assert_eq!(keys.thumbnail.as_deref(), Some(PREVIEW_KEY));
    assert_eq!(keys.full.as_deref(), Some(COMPLETE_KEY));

    let member = RoomMemberRecord {
        room_id: room_id.clone(),
        user_id: peer_id.clone(),
        is_self: false,
        display_name: None,
        handle: None,
        role: "member".to_owned(),
        avatar_thumbnail_key: Some("3-1-new".to_owned()),
        avatar_full_key: None,
    };
    store
        .replace_room_members(&room_id, &[member])
        .await
        .expect("replace");

    let keys = store
        .member_avatar_keys(&peer_id)
        .await
        .expect("member_avatar_keys")
        .expect("a stored member");
    assert_eq!(keys.thumbnail.as_deref(), Some("3-1-new"));
    assert_eq!(keys.full, None);
    assert_eq!(
        store
            .member_avatar_keys("nobody@example.test")
            .await
            .expect("lookup"),
        None,
        "no member row is no answer, unlike a member without an avatar"
    );
}

fn migrations() -> Vec<(u32, std::path::PathBuf)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/storage/migrations");
    let mut files: Vec<(u32, std::path::PathBuf)> = std::fs::read_dir(dir)
        .expect("read migrations")
        .map(|entry| {
            let path = entry.expect("dir entry").path();
            let name = path.file_name().expect("name").to_string_lossy();
            let version = name
                .strip_prefix('V')
                .and_then(|rest| rest.split("__").next())
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| panic!("{name} is no migration"));
            (version, path)
        })
        .collect();
    files.sort();
    files
}

#[test]
fn v34_adds_the_avatar_columns_and_older_rows_read_null() {
    let files = migrations();
    let v34 = files
        .iter()
        .find(|(version, _)| *version == 34)
        .expect("V34 exists");
    let conn = rusqlite::Connection::open_in_memory().expect("memory db");
    for (_, path) in files.iter().filter(|(version, _)| *version <= 33) {
        conn.execute_batch(&std::fs::read_to_string(path).expect("read"))
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }
    conn.execute(
        "INSERT INTO self_user (id, name) VALUES (?1, 'Alice')",
        [QUALIFIED],
    )
    .expect("a V33 row");

    conn.execute_batch(&std::fs::read_to_string(&v34.1).expect("read"))
        .expect("V34 applies over V33");

    let columns: (Option<String>, Option<String>, Option<String>) = conn
        .query_row(
            "SELECT avatar_thumbnail_key, avatar_full_key, team_avatar_key FROM self_user WHERE id = ?1",
            [QUALIFIED],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("the row");
    assert_eq!(columns, (None, None, None));
    conn.prepare("SELECT avatar_thumbnail_key, avatar_full_key FROM room_members")
        .expect("room_members has the columns");
}

// --- the surface -----------------------------------------------------------

fn workspace_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// `step_94_tab_proxy_parity` holds the wasm class and the tab mirror to each
/// other; this holds that both name the five calls, in the planned classes.
#[test]
fn the_browser_surface_carries_the_five_calls() {
    let wasm = workspace_file("kupferdraht-core/src/wasm/manager.rs");
    let tabs = "bindings/web/kupferdraht-tabs";
    let protocol = workspace_file(&format!("{tabs}/protocol.js"));
    let proxy = workspace_file(&format!("{tabs}/proxy.js"));
    let types = workspace_file(&format!("{tabs}/index.d.ts"));
    let calls = [
        ("avatar", "idempotent"),
        ("avatarKey", "idempotent"),
        ("teamAvatar", "idempotent"),
        ("uploadAvatar", "mutation"),
        ("removeAvatar", "mutation"),
    ];
    for (call, class) in calls {
        assert!(
            wasm.contains(&format!("js_name = {call})")),
            "wasm has {call}"
        );
        assert!(
            protocol.contains(&format!(r#"["Client", "{call}", "{class}", null]"#)),
            "protocol.js has {call} as {class}"
        );
        assert!(proxy.contains(&format!("{call}(")), "proxy.js has {call}");
        assert!(types.contains(&format!("{call}(")), "index.d.ts has {call}");
    }
    assert!(
        types.contains("avatarKey: string | null"),
        "Member carries avatarKey"
    );
}

#[test]
fn the_ffi_mirrors_the_five_calls() {
    let ffi = workspace_file("kupferdraht-core/src/ffi/avatars.rs");
    for call in [
        "pub async fn avatar(",
        "pub async fn avatar_key(",
        "pub async fn team_avatar(",
        "pub async fn upload_avatar(",
        "pub async fn remove_avatar(",
    ] {
        assert!(ffi.contains(call), "the FFI Client has {call}");
    }
}

#[test]
fn the_cli_has_the_avatar_commands() {
    let main = workspace_file("kupferdraht-cli/src/main.rs");
    assert!(main.contains("Avatar"), "kupferdraht-cli avatar");
    let rooms = workspace_file("kupferdraht-cli/src/cli/rooms.rs");
    assert!(
        rooms.contains("avatar_key()"),
        "room-members prints the key"
    );
}
