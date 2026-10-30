// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

#![allow(clippy::doc_markdown)]
//! `GET /v{N}/events` — the ConsumableNotifications WebSocket
//! (push-mode receive transport, Step 62a).
//!
//! Transport only: upgrade, frame parse, ack encode, pong. No MLS, no
//! storage, no reconnect policy — the push loop (Step 62b) owns those.
//! The frame and ack shapes below are the frozen live-derived contract
//! in `docs/backend-spec.md` §11.2/§11.3; auth rides in the
//! `access_token` query parameter because Wire strips `Authorization`
//! headers on the upgrade (backend-spec §11.2, LEARNINGS §1.6).
//!
//! Native only (`cfg(not(target_arch = "wasm32"))` at the module
//! registration): the browser cannot set upgrade headers and gets its
//! own transport in a later step; `tokio-tungstenite` must never enter
//! the wasm dependency graph.

use futures_util::{SinkExt as _, StreamExt as _};
use serde::Deserialize;
use tokio_tungstenite::tungstenite::Message;

use crate::error::KupferdrahtError;
// Step 115b: the legacy frame parser moved to a portable module — the browser
// push socket needs it and cannot pull in this module's transport dependencies.
use crate::wire::notification_dto::QueuedNotification;
use crate::wire::push_frame::{check_notification_id, parse_legacy_frame};

/// The tungstenite stream behind an [`EventSocket`]: TCP or
/// rustls-wrapped TCP, negotiated by `connect_async` from the URL
/// scheme.
type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// One inbound WebSocket text frame, parsed. Wire sends exactly one
/// JSON `ConsumableNotification` per text frame (backend-spec §11.2).
#[derive(Debug)]
pub(crate) enum ConsumableNotification {
    /// A queued or live notification. `delivery_tag` is
    /// connection-scoped, strictly increasing from 1, and is the ack
    /// currency (§11.3); the inner notification is shape-identical to
    /// the REST queue's entries, hence the reused DTO.
    Event {
        delivery_tag: u64,
        event: QueuedNotification,
    },
    /// The "caught up" marker: emitted after every notification queued
    /// at connection time has been delivered. `marker_id` echoes the
    /// `sync_marker` query parameter — **of the connection that
    /// requested it**: current Wire backends queue marker events
    /// durably, so a reconnecting client can receive stale markers
    /// from earlier connections first (live-observed on a
    /// self-hosted instance). `delivery_tag` is `Some` on
    /// those backends — the marker is a queue delivery and **must be
    /// acked** like an event, or the stream stalls on the broker's
    /// prefetch window; the frozen spec's tag-less form stays
    /// tolerated (`None`).
    Synchronization {
        marker_id: String,
        delivery_tag: Option<u64>,
    },
    /// A frame type this client does not know. Tolerated by contract
    /// (log + discard, never tear down the socket) so a backend
    /// addition cannot break deployed clients.
    Unknown { frame_type: String },
}

/// Parse one inbound text frame into a [`ConsumableNotification`].
///
/// # Errors
/// `KupferdrahtError::Encoding` when the frame is not JSON, has no `type`
/// field, a known `type` misses its required `data` fields, or carries
/// an empty notification id or a zero delivery tag (Step 95c). An
/// *unknown* `type` is `Ok(Unknown)`, not an error.
pub(crate) fn parse_consumable_frame(
    frame: &str,
) -> Result<ConsumableNotification, KupferdrahtError> {
    let envelope: FrameEnvelope = serde_json::from_str(frame)
        .map_err(|e| KupferdrahtError::Encoding(format!("consumable frame envelope: {e}")))?;
    match envelope.frame_type.as_str() {
        "event" => {
            let data: EventData = serde_json::from_value(envelope.data)
                .map_err(|e| KupferdrahtError::Encoding(format!("consumable `event` data: {e}")))?;
            check_delivery_tag(data.delivery_tag)?;
            check_notification_id(&data.event)?;
            Ok(ConsumableNotification::Event {
                delivery_tag: data.delivery_tag,
                event: data.event,
            })
        }
        "synchronization" => {
            let data: SynchronizationData = serde_json::from_value(envelope.data).map_err(|e| {
                KupferdrahtError::Encoding(format!("consumable `synchronization` data: {e}"))
            })?;
            if let Some(tag) = data.delivery_tag {
                check_delivery_tag(tag)?;
            }
            Ok(ConsumableNotification::Synchronization {
                marker_id: data.marker_id,
                delivery_tag: data.delivery_tag,
            })
        }
        _ => Ok(ConsumableNotification::Unknown {
            frame_type: envelope.frame_type,
        }),
    }
}

/// Step 95c: delivery tags are connection-scoped and 1-based (§11.3) —
/// a zero tag is a malformed frame (it could only be acked wrongly).
fn check_delivery_tag(delivery_tag: u64) -> Result<(), KupferdrahtError> {
    if delivery_tag == 0 {
        return Err(KupferdrahtError::Encoding("frame: delivery_tag 0".into()));
    }
    Ok(())
}

/// The outer `{"type": …, "data": …}` shape every frame shares.
/// `data` defaults to `null` so an unknown type without a `data`
/// member still lands in the tolerated `Unknown` arm.
#[derive(Deserialize)]
struct FrameEnvelope {
    #[serde(rename = "type")]
    frame_type: String,
    #[serde(default)]
    data: serde_json::Value,
}

/// `data` of a `"type":"event"` frame (backend-spec §11.2).
#[derive(Deserialize)]
struct EventData {
    delivery_tag: u64,
    event: QueuedNotification,
}

/// `data` of a `"type":"synchronization"` frame. The frozen spec
/// (backend-spec §11.2) carries only `marker_id`; current Wire
/// backends add a `delivery_tag` (the marker rides the durable queue
/// and is acked like an event — live-observed).
#[derive(Deserialize)]
struct SynchronizationData {
    marker_id: String,
    #[serde(default)]
    delivery_tag: Option<u64>,
}

/// Serialize the §11.3 ack frame:
/// `{"type":"ack","data":{"delivery_tag":<n>,"multiple":<bool>}}`.
/// `multiple: true` acknowledges every delivery up to and including
/// `delivery_tag` on this connection.
pub(crate) fn ack_frame(delivery_tag: u64, multiple: bool) -> String {
    serde_json::json!({
        "type": "ack",
        "data": { "delivery_tag": delivery_tag, "multiple": multiple }
    })
    .to_string()
}

/// Build the upgrade URL:
/// `<websocket_base>/v{N}/events?client=…&sync_marker=…&access_token=…`.
/// `websocket_base` is the per-session `websocket_url` persisted since
/// the V1 migration (`SessionRecord.websocket_url`).
///
/// # Errors
/// `KupferdrahtError::InvalidInput` when `websocket_base` is not a `ws://`
/// or `wss://` URL — the token would otherwise leak into an HTTP
/// request line.
pub(crate) fn events_url(
    websocket_base: &str,
    api_version: u32,
    client_id: &str,
    sync_marker: &str,
    access_token: &str,
) -> Result<url::Url, KupferdrahtError> {
    let trimmed = websocket_base.trim_end_matches('/');
    let mut url = url::Url::parse(&format!("{trimmed}/v{api_version}/events")).map_err(|e| {
        KupferdrahtError::InvalidInput(format!(
            "events_url: bad websocket base `{websocket_base}`: {e}"
        ))
    })?;
    if !matches!(url.scheme(), "ws" | "wss") {
        return Err(KupferdrahtError::InvalidInput(format!(
            "events_url: websocket base must be ws:// or wss://, got `{}`",
            url.scheme()
        )));
    }
    url.query_pairs_mut()
        .append_pair("client", client_id)
        .append_pair("sync_marker", sync_marker)
        .append_pair("access_token", access_token);
    Ok(url)
}

/// Build the legacy cannon `/await` URL (Step 73, ADR-0019): the
/// version-less route Wire keeps for pre-v9 clients — and the one
/// every current wire-server actually pushes through while
/// ConsumableNotifications is upstream-disabled (observations §68;
/// recorded ADR-0008 deviation in `api-notes.md`). `client` scopes
/// the delivery queue to this device. The access token deliberately
/// does NOT ride the URL — it travels as a `Bearer` header
/// ([`EventSocket::connect_with_bearer`]).
pub(crate) fn await_url(
    websocket_base: &str,
    client_id: &str,
) -> Result<url::Url, KupferdrahtError> {
    debug_assert!(
        !client_id.is_empty(),
        "await_url: client_id must be non-empty"
    );
    let trimmed = websocket_base.trim_end_matches('/');
    let mut url = url::Url::parse(&format!("{trimmed}/await")).map_err(|e| {
        KupferdrahtError::InvalidInput(format!(
            "await_url: bad websocket base `{websocket_base}`: {e}"
        ))
    })?;
    if !matches!(url.scheme(), "ws" | "wss") {
        return Err(KupferdrahtError::InvalidInput(format!(
            "await_url: websocket base must be ws:// or wss://, got `{}`",
            url.scheme()
        )));
    }
    url.query_pairs_mut().append_pair("client", client_id);
    Ok(url)
}

/// A connected `/v{N}/events` socket. Thin: read parsed frames, write
/// acks, answer server pings. One instance per connection — the
/// backend terminates the previous socket when the same client
/// connects again (§11.2), so reconnecting means a fresh
/// [`EventSocket`].
pub(crate) struct EventSocket {
    inner: WsStream,
}

impl EventSocket {
    /// Open the WebSocket at `url` (built by [`events_url`]) and
    /// complete the upgrade.
    ///
    /// # Errors
    /// `KupferdrahtError::Network` on TCP/TLS/upgrade failure.
    ///
    /// # Panics
    /// Panics when `url` is not `ws`/`wss` — callers must build it
    /// with [`events_url`], which enforces the scheme (the token in
    /// the query must never travel over plain HTTP).
    pub(crate) async fn connect(url: url::Url) -> Result<Self, KupferdrahtError> {
        assert!(
            matches!(url.scheme(), "ws" | "wss"),
            "EventSocket::connect: non-websocket url — build it with events_url"
        );
        let (inner, response) = tokio_tungstenite::connect_async(url.as_str())
            .await
            .map_err(|e| KupferdrahtError::Network(format!("events websocket connect: {e}")))?;
        // 101 is the only status connect_async resolves Ok with; pin
        // the §11.2 upgrade contract explicitly.
        debug_assert_eq!(
            response.status(),
            tokio_tungstenite::tungstenite::http::StatusCode::SWITCHING_PROTOCOLS
        );
        Ok(Self { inner })
    }

    /// Open the legacy `/await` WebSocket (Step 73, ADR-0019): the
    /// access token travels as an `Authorization: Bearer` header on
    /// the upgrade request — never in the URL (which is why this is
    /// a separate constructor from [`Self::connect`]).
    ///
    /// # Errors
    /// `KupferdrahtError::Network` on TCP/TLS/upgrade failure;
    /// `KupferdrahtError::InvalidInput` for a malformed header value.
    ///
    /// # Panics
    /// Panics when `url` is not `ws`/`wss` — callers must build it
    /// with [`await_url`], which enforces the scheme.
    pub(crate) async fn connect_with_bearer(
        url: url::Url,
        access_token: &str,
    ) -> Result<Self, KupferdrahtError> {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
        assert!(
            matches!(url.scheme(), "ws" | "wss"),
            "EventSocket::connect_with_bearer: non-websocket url — build it with await_url"
        );
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| KupferdrahtError::InvalidInput(format!("await websocket request: {e}")))?;
        let header = format!("Bearer {access_token}").parse().map_err(|e| {
            KupferdrahtError::InvalidInput(format!("await websocket bearer header: {e}"))
        })?;
        request.headers_mut().insert("Authorization", header);
        let (inner, response) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|e| KupferdrahtError::Network(format!("await websocket connect: {e}")))?;
        debug_assert_eq!(
            response.status(),
            tokio_tungstenite::tungstenite::http::StatusCode::SWITCHING_PROTOCOLS
        );
        Ok(Self { inner })
    }

    /// Read the next legacy `/await` frame (Step 73): each text or
    /// binary frame carries one `QueuedNotification` — the exact
    /// `/v14/notifications` item shape (observations §68). Server
    /// pings are answered internally; `Ok(None)` on an orderly close.
    ///
    /// # Cancel safety
    /// This future is polled inside `tokio::select!` against the
    /// keepalive tick and may be **dropped at any await point**. The
    /// `/await` route has no redelivery, so a data frame held across
    /// an await would be lost forever. The invariant this body must
    /// keep: the **only** await is `self.inner.next()`, and from the
    /// moment a data message is consumed everything up to `return`
    /// is synchronous (parse included) — never add an await (e.g. an
    /// explicit pong reply) between consuming a message and
    /// returning it. Buffered frames live in `self.inner`, which the
    /// future only borrows, so a dropped future loses nothing.
    ///
    /// # Errors
    /// `KupferdrahtError::Network` on transport failure,
    /// `KupferdrahtError::Encoding` on non-UTF-8 bytes or malformed JSON.
    pub(crate) async fn next_legacy_frame(
        &mut self,
    ) -> Result<Option<crate::wire::notification_dto::QueuedNotification>, KupferdrahtError> {
        loop {
            let Some(message) = self.inner.next().await else {
                return Ok(None);
            };
            let message = message
                .map_err(|e| KupferdrahtError::Network(format!("await websocket read: {e}")))?;
            let text: String = match message {
                Message::Text(text) => text.to_string(),
                Message::Binary(bytes) => std::str::from_utf8(&bytes)
                    .map_err(|e| {
                        KupferdrahtError::Encoding(format!(
                            "await binary frame is not UTF-8 JSON: {e}"
                        ))
                    })?
                    .to_string(),
                Message::Close(_) => return Ok(None),
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => continue,
            };
            return parse_legacy_frame(&text).map(Some);
        }
    }

    /// Send a WebSocket ping (Step 73): the legacy route has no
    /// application-level traffic from the client, so periodic pings
    /// keep cannon's idle timeout from closing a healthy socket.
    ///
    /// # Errors
    /// `KupferdrahtError::Network` on transport failure.
    pub(crate) async fn send_ping(&mut self) -> Result<(), KupferdrahtError> {
        self.inner
            .send(Message::Ping(Vec::new().into()))
            .await
            .map_err(|e| KupferdrahtError::Network(format!("await websocket ping: {e}")))
    }

    /// Read the next frame. Server pings are answered internally and
    /// never surface. Returns `Ok(None)` on an orderly close.
    ///
    /// # Errors
    /// `KupferdrahtError::Network` on transport failure,
    /// `KupferdrahtError::Encoding` on an unparseable text frame.
    pub(crate) async fn next_frame(
        &mut self,
    ) -> Result<Option<ConsumableNotification>, KupferdrahtError> {
        loop {
            let Some(message) = self.inner.next().await else {
                // Stream exhausted without a close frame — the peer
                // vanished; 62b's loop treats this like a close.
                return Ok(None);
            };
            let message = message
                .map_err(|e| KupferdrahtError::Network(format!("events websocket read: {e}")))?;
            match message {
                Message::Text(text) => return parse_consumable_frame(text.as_str()).map(Some),
                // Current Wire backends deliver the same JSON as
                // binary frames (live-observed on a self-hosted
                // instance); the frozen spec's text form stays
                // accepted above. Non-UTF-8 bytes are a protocol
                // break, same class as malformed JSON.
                Message::Binary(bytes) => {
                    let text = std::str::from_utf8(&bytes).map_err(|e| {
                        KupferdrahtError::Encoding(format!(
                            "consumable binary frame is not UTF-8 JSON: {e}"
                        ))
                    })?;
                    return parse_consumable_frame(text).map(Some);
                }
                Message::Close(_) => return Ok(None),
                // Ping/Pong: tungstenite queues the pong reply itself
                // and flushes it on this very read loop. Frame: raw
                // frames never surface from a message-level read. None
                // of the three carries a ConsumableNotification.
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
    }

    /// Send one §11.3 ack. The caller (62b's push loop) must have
    /// durably processed the acked delivery first — an acked
    /// notification is consumed server-side and never redelivered.
    ///
    /// # Errors
    /// `KupferdrahtError::Network` on transport failure.
    pub(crate) async fn send_ack(
        &mut self,
        delivery_tag: u64,
        multiple: bool,
    ) -> Result<(), KupferdrahtError> {
        debug_assert!(
            delivery_tag >= 1,
            "delivery tags are connection-scoped and 1-based (backend-spec §11.3)"
        );
        self.inner
            .send(Message::text(ack_frame(delivery_tag, multiple)))
            .await
            .map_err(|e| KupferdrahtError::Network(format!("events websocket ack: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};

    use super::{
        ConsumableNotification, EventSocket, ack_frame, events_url, parse_consumable_frame,
        parse_legacy_frame,
    };
    use crate::error::KupferdrahtError;

    /// Known-answer frames, verbatim from backend-spec §11.2 (frozen
    /// live-derived contract).
    const EVENT_FRAME: &str = r#"{"type":"event","data":{"delivery_tag":1,"event":{"id":"5f7ee184-b0d1-11ee-8000-000000000000","payload":[{"type":"conversation.typing","data":{"status":"started"}}]}}}"#;
    const SYNC_FRAME: &str = r#"{"type":"synchronization","data":{"marker_id":"marker-uuid-1"}}"#;

    // ── frame parsing (§11.2) ──────────────────────────────────────

    /// Step 95c: a frame's notification `id` becomes the persisted
    /// cursor and the dedup key, and `delivery_tag` is 1-based per
    /// connection (§11.3). An empty id or a zero tag used to reach
    /// `debug_assert!`s in the push loop — backend data. Both are
    /// malformed frames now, on both routes.
    #[test]
    fn frames_with_an_empty_id_or_a_zero_tag_are_malformed() {
        let hostile_consumable = [
            r#"{"type":"event","data":{"delivery_tag":1,"event":{"id":"","payload":[]}}}"#,
            r#"{"type":"event","data":{"delivery_tag":0,"event":{"id":"5f7ee184-b0d1-11ee-8000-000000000000","payload":[]}}}"#,
            r#"{"type":"synchronization","data":{"marker_id":"marker-uuid-1","delivery_tag":0}}"#,
        ];
        for frame in hostile_consumable {
            assert!(
                matches!(
                    parse_consumable_frame(frame),
                    Err(KupferdrahtError::Encoding(_))
                ),
                "{frame}"
            );
        }
        assert!(matches!(
            parse_legacy_frame(r#"{"id":"","payload":[]}"#),
            Err(KupferdrahtError::Encoding(_))
        ));
    }

    #[test]
    fn event_frame_parses_delivery_tag_and_notification() {
        let parsed = parse_consumable_frame(EVENT_FRAME).expect("spec frame parses");
        let ConsumableNotification::Event {
            delivery_tag,
            event,
        } = parsed
        else {
            panic!("expected Event variant");
        };
        assert_eq!(delivery_tag, 1);
        assert_eq!(event.id, "5f7ee184-b0d1-11ee-8000-000000000000");
        let payload = event.payload.as_array().expect("payload is an array");
        assert_eq!(payload.len(), 1);
        assert_eq!(payload[0]["type"], "conversation.typing");
    }

    #[test]
    fn synchronization_frame_parses_marker_id() {
        let parsed = parse_consumable_frame(SYNC_FRAME).expect("spec frame parses");
        let ConsumableNotification::Synchronization {
            marker_id,
            delivery_tag,
        } = parsed
        else {
            panic!("expected Synchronization variant");
        };
        assert_eq!(marker_id, "marker-uuid-1");
        assert_eq!(delivery_tag, None, "the frozen-spec form carries no tag");
    }

    #[test]
    fn synchronization_frame_with_delivery_tag_parses_live_shape() {
        // Verbatim from a live run against the
        // self-hosted instance: current backends queue the marker
        // durably — it carries a delivery_tag and `data` precedes
        // `type` in the serialized order.
        let live = r#"{"data":{"delivery_tag":1,"marker_id":"1680ae08-7c87-461d-9198-bd019cd33844"},"type":"synchronization"}"#;
        let parsed = parse_consumable_frame(live).expect("live frame parses");
        let ConsumableNotification::Synchronization {
            marker_id,
            delivery_tag,
        } = parsed
        else {
            panic!("expected Synchronization variant");
        };
        assert_eq!(marker_id, "1680ae08-7c87-461d-9198-bd019cd33844");
        assert_eq!(delivery_tag, Some(1));
    }

    #[test]
    fn unknown_frame_type_is_tolerated_not_error() {
        let parsed = parse_consumable_frame(r#"{"type":"message_count","data":{"count":7}}"#)
            .expect("unknown type is tolerated, not an error");
        let ConsumableNotification::Unknown { frame_type } = parsed else {
            panic!("expected Unknown variant");
        };
        assert_eq!(frame_type, "message_count");
    }

    #[test]
    fn malformed_event_frame_is_encoding_error() {
        // Known type, required `data` fields missing.
        let err = parse_consumable_frame(r#"{"type":"event","data":{}}"#)
            .expect_err("missing delivery_tag/event must not parse");
        assert!(matches!(err, KupferdrahtError::Encoding(_)), "got: {err:?}");
    }

    // ── ack encoding (§11.3) ───────────────────────────────────────

    #[test]
    fn ack_frame_matches_spec_shape() {
        let frame: serde_json::Value =
            serde_json::from_str(&ack_frame(7, false)).expect("ack frame is JSON");
        assert_eq!(
            frame,
            serde_json::json!({
                "type": "ack",
                "data": { "delivery_tag": 7, "multiple": false }
            })
        );
    }

    // ── upgrade URL (§11.2 auth contract) ──────────────────────────

    #[test]
    fn events_url_appends_version_path_and_auth_query() {
        let url = events_url("wss://ws.example.test", 14, "client-1", "marker-1", "tok-1")
            .expect("wss base builds");
        assert_eq!(url.scheme(), "wss");
        assert_eq!(url.path(), "/v14/events");
        let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(query["client"], "client-1");
        assert_eq!(query["sync_marker"], "marker-1");
        // Auth MUST ride in the query — Wire strips Authorization
        // headers on the upgrade (backend-spec §11.2, LEARNINGS §1.6).
        assert_eq!(query["access_token"], "tok-1");
    }

    #[test]
    fn events_url_rejects_non_ws_scheme() {
        let err = events_url("https://ws.example.test", 14, "c", "m", "t")
            .expect_err("http(s) base must be rejected");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "got: {err:?}"
        );
    }

    #[test]
    fn await_url_is_versionless_with_client_param_only() {
        let url = super::await_url("wss://ws.example.test/", "client-1").expect("wss base builds");
        assert_eq!(url.scheme(), "wss");
        // Version-less by contract — the versioned `/v{N}/await`
        // variants 404 at nginx (api-notes, Step 73 deviation).
        assert_eq!(url.path(), "/await");
        let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(query["client"], "client-1");
        // The token travels as a Bearer header on this route
        // (ADR-0019) — it must NEVER appear in the URL.
        assert_eq!(query.len(), 1, "unexpected query params: {query:?}");
        assert!(!url.as_str().contains("access_token"));
    }

    #[test]
    fn await_url_rejects_non_ws_scheme() {
        let err = super::await_url("https://ws.example.test", "c")
            .expect_err("http(s) base must be rejected");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "got: {err:?}"
        );
    }

    // ── live socket against an in-process server ───────────────────
    // httpmock cannot speak WebSocket; these tests run a loopback
    // tokio-tungstenite server instead.

    async fn bind_loopback() -> (tokio::net::TcpListener, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let base = format!("ws://{}", listener.local_addr().expect("local addr"));
        (listener, base)
    }

    #[tokio::test]
    async fn connect_sends_client_marker_and_token_query_params_on_upgrade() {
        let (listener, base) = bind_loopback().await;
        let (uri_tx, uri_rx) = tokio::sync::oneshot::channel::<String>();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            // The Err size is tungstenite's Callback trait contract
            // (Result<Response, ErrorResponse>) — not ours to shrink.
            #[allow(clippy::result_large_err)]
            let capture = move |req: &Request, resp: Response| {
                uri_tx.send(req.uri().to_string()).expect("send uri");
                Ok(resp)
            };
            // Keep the socket open until the client is done.
            let _ws = tokio_tungstenite::accept_hdr_async(stream, capture)
                .await
                .expect("upgrade");
        });

        let url = events_url(&base, 14, "client-1", "marker-1", "tok-1").expect("url");
        let _socket = EventSocket::connect(url).await.expect("connect");

        let uri = uri_rx.await.expect("server captured the request URI");
        assert!(uri.starts_with("/v14/events?"), "uri: {uri}");
        assert!(uri.contains("client=client-1"), "uri: {uri}");
        assert!(uri.contains("sync_marker=marker-1"), "uri: {uri}");
        assert!(uri.contains("access_token=tok-1"), "uri: {uri}");
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn frames_arrive_in_order_event_then_synchronization() {
        let (listener, base) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("upgrade");
            ws.send(Message::text(EVENT_FRAME))
                .await
                .expect("send event");
            ws.send(Message::text(SYNC_FRAME)).await.expect("send sync");
            ws.close(None).await.expect("close");
        });

        let url = events_url(&base, 14, "c", "m", "t").expect("url");
        let mut socket = EventSocket::connect(url).await.expect("connect");

        let first = socket.next_frame().await.expect("first frame");
        assert!(
            matches!(
                first,
                Some(ConsumableNotification::Event {
                    delivery_tag: 1,
                    ..
                })
            ),
            "got: {first:?}"
        );
        let second = socket.next_frame().await.expect("second frame");
        assert!(
            matches!(second, Some(ConsumableNotification::Synchronization { .. })),
            "got: {second:?}"
        );
        let end = socket.next_frame().await.expect("orderly close");
        assert!(end.is_none(), "got: {end:?}");
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn binary_frames_carry_the_same_json_contract() {
        // Current Wire backends deliver the JSON as binary frames
        // (live-observed); the client accepts both.
        let (listener, base) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("upgrade");
            ws.send(Message::binary(EVENT_FRAME.as_bytes()))
                .await
                .expect("send binary event frame");
            ws.close(None).await.expect("close");
        });

        let url = events_url(&base, 14, "c", "m", "t").expect("url");
        let mut socket = EventSocket::connect(url).await.expect("connect");
        let first = socket.next_frame().await.expect("first frame");
        assert!(
            matches!(
                first,
                Some(ConsumableNotification::Event {
                    delivery_tag: 1,
                    ..
                })
            ),
            "got: {first:?}"
        );
        server.await.expect("server task");
    }

    #[tokio::test]
    async fn send_ack_reaches_server_verbatim() {
        let (listener, base) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("upgrade");
            let msg = ws.next().await.expect("one frame").expect("ok frame");
            let ack: serde_json::Value =
                serde_json::from_str(msg.to_text().expect("text frame")).expect("json");
            assert_eq!(
                ack,
                serde_json::json!({
                    "type": "ack",
                    "data": { "delivery_tag": 1, "multiple": false }
                })
            );
        });

        let url = events_url(&base, 14, "c", "m", "t").expect("url");
        let mut socket = EventSocket::connect(url).await.expect("connect");
        socket.send_ack(1, false).await.expect("ack sent");
        server.await.expect("server task asserts the ack shape");
    }

    #[tokio::test]
    async fn server_ping_is_ponged_automatically() {
        let (listener, base) = bind_loopback().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("upgrade");
            ws.send(Message::Ping(b"heartbeat".as_slice().into()))
                .await
                .expect("send ping");
            let reply = ws.next().await.expect("one frame").expect("ok frame");
            assert!(
                matches!(&reply, Message::Pong(p) if p.as_ref() == b"heartbeat"),
                "got: {reply:?}"
            );
            ws.close(None).await.expect("close");
        });

        let url = events_url(&base, 14, "c", "m", "t").expect("url");
        let mut socket = EventSocket::connect(url).await.expect("connect");
        // The pong must happen inside the read path — the caller only
        // ever sees the orderly close.
        let end = socket.next_frame().await.expect("read to close");
        assert!(end.is_none(), "got: {end:?}");
        server.await.expect("server task saw the pong");
    }
}
