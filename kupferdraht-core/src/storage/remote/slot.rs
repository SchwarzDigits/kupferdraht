// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 120 (ADR-0040): the account's storage slot — the user's one remote
//! device on the storage server, labelled with its Wire client id. Before a
//! recovery Kupferdraht asks whether there is a device and which
//! (`GET /v1/slot`, with the PIN token); after claiming the slot for a new
//! device it deletes the replaced device's Wire client.

use crate::client::DeviceInfo;
use crate::error::KupferdrahtError;

/// The account's slot: the device that holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Slot {
    /// The device's Wire client id; empty while the device is being set up
    /// (claimed, not yet labelled).
    pub(crate) label: String,
    /// When the device claimed the slot, ms since the Unix epoch.
    pub(crate) claimed_at_ms: u64,
}

/// The slot query's URL from the storage WebSocket's: `https` for `wss`
/// (`http` for `ws`), `/v1/slot` for `/v1/ws`.
///
/// # Errors
/// `InvalidInput` for a URL that is not a storage WebSocket URL.
pub(crate) fn slot_url(websocket_url: &str) -> Result<String, KupferdrahtError> {
    let base = websocket_url.strip_suffix("/v1/ws").ok_or_else(|| {
        KupferdrahtError::InvalidInput(format!(
            "storage URL {websocket_url:?}: expected the storage WebSocket (…/v1/ws)"
        ))
    })?;
    let http = if let Some(host) = base.strip_prefix("wss://") {
        format!("https://{host}")
    } else if let Some(host) = base.strip_prefix("ws://") {
        format!("http://{host}")
    } else {
        return Err(KupferdrahtError::InvalidInput(format!(
            "storage URL {websocket_url:?}: expected wss:// (ws:// in development)"
        )));
    };
    Ok(format!("{http}/v1/slot"))
}

/// The account's slot, asked with the PIN token: `None` only when the
/// server says there is none (`200 {"slot":null}`).
///
/// # Errors
/// `StorageAccessDenied` when the server refuses the token (`401`);
/// `ServiceUnavailable` for any other answer or none — never "no device",
/// which would replace the real one.
pub(crate) async fn fetch_slot(
    slot_url: &str,
    permit_http: bool,
    pin_token: &str,
) -> Result<Option<Slot>, KupferdrahtError> {
    if !(slot_url.starts_with("https://") || (permit_http && slot_url.starts_with("http://"))) {
        return Err(KupferdrahtError::InvalidInput(format!(
            "slot query {slot_url}: HTTPS required (plain HTTP only with permitHttp)"
        )));
    }
    let unavailable =
        |what: String| KupferdrahtError::ServiceUnavailable(format!("slot query: {what}"));
    let response = super::token_exchange::http_client()?
        .get(slot_url)
        .bearer_auth(pin_token)
        .send()
        .await
        .map_err(|e| unavailable(e.to_string()))?;
    match response.status().as_u16() {
        200 => {}
        401 => {
            return Err(KupferdrahtError::StorageAccessDenied(
                "the storage server refused the token for the slot query".into(),
            ));
        }
        status => return Err(unavailable(format!("the storage server answered {status}"))),
    }
    let body: SlotAnswer = response
        .json()
        .await
        .map_err(|e| unavailable(format!("unreadable answer: {e}")))?;
    Ok(body.slot.map(|slot| Slot {
        label: slot.label,
        claimed_at_ms: slot.claimed_at_ms,
    }))
}

/// `GET /v1/slot`'s answer.
#[derive(serde::Deserialize)]
struct SlotAnswer {
    slot: Option<SlotBody>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SlotBody {
    label: String,
    claimed_at_ms: u64,
}

/// Whether recovery data could bring the account back: the slot's device
/// is set up — it has a label. A device being set up has none yet.
pub(crate) fn recovery_expected(slot: Option<&Slot>) -> bool {
    slot.is_some_and(|slot| !slot.label.is_empty())
}

/// The Wire clients a new setup deletes once it holds the slot: the
/// replaced device's, if it is a Kupferdraht web device; with no label to go by
/// (`None`: the old device stopped during its setup — ask only when the
/// slot existed) every Kupferdraht web device of the user but `own`. Never
/// `own`, never another kind of client.
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) fn clients_to_retire(
    clients: &[DeviceInfo],
    replaced: Option<&str>,
    own: &str,
) -> Vec<String> {
    let web_device = |client: &&DeviceInfo| {
        client.model.as_deref() == Some(crate::wire::WEB_DEVICE_MODEL) && client.device_id != own
    };
    clients
        .iter()
        .filter(web_device)
        .filter(|client| replaced.is_none_or(|id| client.device_id == id))
        .map(|client| client.device_id.clone())
        .collect()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use httpmock::Method::GET;
    use httpmock::MockServer;

    use super::*;
    use crate::wire::WEB_DEVICE_MODEL;

    #[test]
    fn the_slot_url_follows_the_websocket_url() {
        assert_eq!(
            slot_url("wss://storage.example.test/v1/ws").expect("url"),
            "https://storage.example.test/v1/slot"
        );
        assert_eq!(
            slot_url("ws://localhost:9000/v1/ws").expect("url"),
            "http://localhost:9000/v1/slot"
        );
        for other in [
            "https://storage.example.test/v1/ws",
            "wss://storage.example.test/",
            "",
        ] {
            assert!(
                matches!(slot_url(other), Err(KupferdrahtError::InvalidInput(_))),
                "{other:?}"
            );
        }
    }

    fn answer(server: &MockServer, status: u16, body: &str) {
        server.mock(|when, then| {
            when.method(GET)
                .path("/v1/slot")
                .header("authorization", "Bearer pin.token");
            then.status(status)
                .header("content-type", "application/json")
                .body(body);
        });
    }

    async fn slot(server: &MockServer) -> Result<Option<Slot>, KupferdrahtError> {
        fetch_slot(&format!("{}/v1/slot", server.base_url()), true, "pin.token").await
    }

    #[tokio::test]
    async fn a_device_holds_the_slot() {
        let server = MockServer::start();
        answer(
            &server,
            200,
            r#"{"slot":{"label":"db8ad281d4aa66","claimedAtMs":1791000000000}}"#,
        );

        assert_eq!(
            slot(&server).await.expect("slot"),
            Some(Slot {
                label: "db8ad281d4aa66".into(),
                claimed_at_ms: 1_791_000_000_000,
            })
        );
    }

    #[tokio::test]
    async fn no_device_is_only_an_explicit_null() {
        let server = MockServer::start();
        answer(&server, 200, r#"{"slot":null}"#);

        assert_eq!(slot(&server).await.expect("slot"), None);
    }

    #[tokio::test]
    async fn a_device_being_set_up_has_an_empty_label() {
        let server = MockServer::start();
        answer(&server, 200, r#"{"slot":{"label":"","claimedAtMs":1}}"#);

        let slot = slot(&server).await.expect("slot").expect("a slot");
        assert!(slot.label.is_empty());
        assert!(!recovery_expected(Some(&slot)));
    }

    #[tokio::test]
    async fn a_refused_token_is_no_storage_access() {
        let server = MockServer::start();
        answer(&server, 401, r#"{"error":"invalid_token"}"#);

        assert!(matches!(
            slot(&server).await,
            Err(KupferdrahtError::StorageAccessDenied(_))
        ));
    }

    /// Anything but `200` is "unknown", never "no device".
    #[tokio::test]
    async fn any_other_answer_is_unavailable_not_no_device() {
        for status in [404, 500, 503] {
            let server = MockServer::start();
            answer(&server, status, "{}");
            assert!(
                matches!(
                    slot(&server).await,
                    Err(KupferdrahtError::ServiceUnavailable(_))
                ),
                "{status}"
            );
        }
        let unreachable = fetch_slot("http://127.0.0.1:9/v1/slot", true, "pin.token").await;
        assert!(matches!(
            unreachable,
            Err(KupferdrahtError::ServiceUnavailable(_))
        ));
    }

    #[test]
    fn recovery_is_expected_for_a_labelled_device() {
        let device = Slot {
            label: "db8ad281d4aa66".into(),
            claimed_at_ms: 1,
        };
        assert!(recovery_expected(Some(&device)));
        assert!(!recovery_expected(None));
    }

    fn client(id: &str, model: &str) -> DeviceInfo {
        DeviceInfo {
            device_id: id.into(),
            class: None,
            label: None,
            model: Some(model.into()),
            registered_at: None,
            last_active: None,
            is_self: false,
        }
    }

    fn clients() -> Vec<DeviceInfo> {
        vec![
            client("aaaa", WEB_DEVICE_MODEL),
            client("bbbb", WEB_DEVICE_MODEL),
            client("cccc", "iPhone"),
            client("own1", WEB_DEVICE_MODEL),
        ]
    }

    #[test]
    fn the_replaced_web_device_is_retired() {
        assert_eq!(
            clients_to_retire(&clients(), Some("aaaa"), "own1"),
            ["aaaa"]
        );
    }

    #[test]
    fn a_replaced_client_of_another_kind_or_unknown_stays() {
        assert!(clients_to_retire(&clients(), Some("cccc"), "own1").is_empty());
        assert!(clients_to_retire(&clients(), Some("dddd"), "own1").is_empty());
    }

    /// The old device stopped before it labelled the slot: every other web
    /// device is an orphan.
    #[test]
    fn without_a_label_every_other_web_device_is_retired() {
        let mut retired = clients_to_retire(&clients(), None, "own1");
        retired.sort();
        assert_eq!(retired, ["aaaa", "bbbb"]);
    }

    #[test]
    fn the_own_device_is_never_retired() {
        assert!(clients_to_retire(&clients(), Some("own1"), "own1").is_empty());
    }
}
