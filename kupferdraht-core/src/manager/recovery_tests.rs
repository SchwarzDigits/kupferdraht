// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 112b (ADR-0035) — recovery data in a seed home.
//!
//! An account's recovery (`enable`, `changePin`, `verifyPin`, the state)
//! and the restore in a new browser (`PendingLogin::recover`), driven
//! natively: each "browser" is a home with its own keystore; the Wire
//! backend is the login mocks; the token exchange and the PIN service are
//! mocks too — the PIN service evaluates every blinded PIN with a known
//! key and counts the attempts, as the real one does.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use httpmock::Method::{GET, POST};
use httpmock::{HttpMockRequest, HttpMockResponse, MockServer};
use serde_json::json;
use tempfile::TempDir;

use super::{KeysState, Kupferdraht, PendingLogin, RegisterOptions};
use crate::client::{Client, ServerConfig};
use crate::error::KupferdrahtError;
use crate::keystore::{InMemoryKeyStore, SecureKeyStore, key_id};
use crate::recovery::account::{
    RecoveryState, change_recovery_pin, enable_recovery, recovery_state, verify_recovery_pin,
};
use crate::recovery::data::{RecoveryData, account_hash};
use crate::recovery::flow::RecoveryServices;
use crate::recovery::oprf::evaluate_as_service;
use crate::storage::account_keys::AccountKeySource;
use crate::storage::remote::wire_session::TokenExchange;

use super::login_mocks::{mount_device_registration, mount_login};

/// The account the login mocks authenticate (`/self`).
const ACCOUNT: &str = "user-uuid@example.test";
const PIN: &str = "482913";
/// The mock PIN service's OPRF key.
const SERVICE_KEY: [u8; 32] = [0x42; 32];
/// The mock PIN service's daily limit.
const DAILY_ATTEMPTS: u32 = 12;

const FIRST_SETUP: RegisterOptions = RegisterOptions { new_setup: true };
const NATIVE_PATH: RegisterOptions = RegisterOptions { new_setup: false };

/// The client id the login mocks register a device under.
const CLIENT: &str = "new-client-id";

/// The token exchange, the PIN service and the storage server's slot query
/// of the tests. The slot query shares the exchange's mock server: httpmock
/// serves from a bounded pool, and parallel tests that hold more servers
/// than it has wait for each other for good.
struct Services {
    exchange: MockServer,
    pin_service: MockServer,
    evaluations: Arc<AtomicU32>,
}

impl Services {
    /// Step 120 (ADR-0040): the account's slot names the login mocks'
    /// device — the one a first setup registers.
    fn start() -> Self {
        Self::with_slot(&format!(
            r#"{{"slot":{{"label":"{CLIENT}","claimedAtMs":1}}}}"#
        ))
    }

    /// The services with the slot query answering `slot_body`.
    fn with_slot(slot_body: &str) -> Self {
        let exchange = MockServer::start();
        let body = slot_body.to_owned();
        exchange.mock(move |when, then| {
            when.method(GET)
                .path("/v1/slot")
                .header("authorization", "Bearer pin.jwt");
            then.status(200)
                .header("content-type", "application/json")
                .body(body);
        });
        exchange.mock(|when, then| {
            when.method(POST)
                .path("/v1/token")
                .json_body(json!({"audience": "pin"}));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"token":"pin.jwt","expiresIn":600}"#);
        });
        let pin_service = MockServer::start();
        let evaluations = Arc::new(AtomicU32::new(0));
        let counter = evaluations.clone();
        pin_service.mock(|when, then| {
            when.method(POST)
                .path("/v1/evaluate")
                .header("authorization", "Bearer pin.jwt");
            then.respond_with(move |request: &HttpMockRequest| {
                let count = counter.fetch_add(1, Ordering::SeqCst) + 1;
                let body: serde_json::Value =
                    serde_json::from_slice(request.body_ref()).expect("a JSON body");
                let blinded = STANDARD
                    .decode(body["blindedElement"].as_str().expect("an element"))
                    .expect("base64");
                let evaluated = evaluate_as_service(&SERVICE_KEY, &blinded);
                HttpMockResponse::builder()
                    .status(200)
                    .header("content-type", "application/json")
                    .body(
                        json!({
                            "keyVersion": 1,
                            "evaluatedElement": STANDARD.encode(evaluated),
                            "attemptsRemaining": DAILY_ATTEMPTS.saturating_sub(count),
                            "attemptId": "AAECAwQFBgcICQoLDA0ODw==",
                        })
                        .to_string(),
                    )
                    .build()
            });
        });
        // Step 124: receipts are accepted (their signatures are checked in
        // `recovery::flow`'s tests).
        pin_service.mock(|when, then| {
            when.method(POST).path("/v1/refund");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"attemptsRemaining":12}"#);
        });
        Self {
            exchange,
            pin_service,
            evaluations,
        }
    }

    fn recovery_services(&self) -> RecoveryServices {
        RecoveryServices {
            exchange: TokenExchange {
                url: self.exchange.base_url(),
                permit_http: true,
            },
            pin_service_url: self.pin_service.base_url(),
            slot_url: format!("{}/v1/slot", self.exchange.base_url()),
        }
    }

    fn evaluations(&self) -> u32 {
        self.evaluations.load(Ordering::SeqCst)
    }
}

/// One browser: its own home, keystore and Wire backend mocks.
struct Browser {
    dir: TempDir,
    keystore: Arc<dyn SecureKeyStore>,
    kupferdraht: Kupferdraht,
    wire: MockServer,
}

impl Browser {
    fn new(services: Option<&Services>) -> Self {
        Self::with_registration(services, true)
    }

    /// A browser whose device registration the backend refuses: the
    /// account's device cap is reached.
    fn at_device_cap(services: &Services) -> Self {
        Self::with_registration(Some(services), false)
    }

    fn with_registration(services: Option<&Services>, registers: bool) -> Self {
        let dir = TempDir::new().expect("tempdir");
        let keystore: Arc<dyn SecureKeyStore> = Arc::new(InMemoryKeyStore::new());
        let mut builder = Kupferdraht::builder()
            .base_dir(dir.path().to_path_buf())
            .keystore(keystore.clone())
            .permit_http()
            .account_keys(AccountKeySource::Seed);
        if let Some(services) = services {
            builder = builder.recovery_services(services.recovery_services());
        }
        let wire = MockServer::start();
        mount_login(&wire);
        wire.mock(|when, then| {
            when.method(GET).path("/v14/clients");
            then.status(200)
                .header("content-type", "application/json")
                .body("[]");
        });
        if registers {
            mount_device_registration(&wire);
        } else {
            wire.mock(|when, then| {
                when.method(POST).path("/v14/clients");
                then.status(400)
                    .header("content-type", "application/json")
                    .body(
                        r#"{"code":400,"label":"too-many-clients","message":"too many clients"}"#,
                    );
            });
        }
        Self {
            dir,
            keystore,
            kupferdraht: builder.build().expect("kupferdraht"),
            wire,
        }
    }

    async fn login(&self) -> Arc<PendingLogin> {
        let config = ServerConfig {
            api_base_url: self.wire.base_url(),
            websocket_url: self.wire.base_url(),
            title: "mock".into(),
        };
        self.kupferdraht
            .login(config, "alice@example.test".into(), "pw".into(), None, None)
            .await
            .expect("login")
    }

    /// Log in and set the account up in this browser.
    async fn first_setup(&self) -> Arc<Client> {
        self.login()
            .await
            .register_device_with(FIRST_SETUP)
            .await
            .expect("first setup")
    }

    fn account_dir(&self) -> std::path::PathBuf {
        self.dir.path().join("accounts").join(ACCOUNT)
    }

    fn seed(&self) -> Option<Vec<u8>> {
        self.keystore
            .get_secret(key_id::account_seed(self.kupferdraht.home_id(), ACCOUNT))
            .expect("keystore read")
    }
}

/// Browser A with the account set up and recovery enabled: its client and
/// the recovery data.
async fn enabled(services: &Services) -> (Browser, Arc<Client>, Vec<u8>) {
    let browser = Browser::new(Some(services));
    let client = browser.first_setup().await;
    let data = enable_recovery(&client, &browser.kupferdraht.recovery_context(), PIN)
        .await
        .expect("enabled");
    (browser, client, data)
}

#[tokio::test]
async fn enable_gives_the_accounts_recovery_data_and_the_state_enabled() {
    let services = Services::start();
    let browser = Browser::new(Some(&services));
    let client = browser.first_setup().await;
    assert_eq!(
        recovery_state(&client).await.expect("state"),
        RecoveryState::Disabled
    );

    let data = enable_recovery(&client, &browser.kupferdraht.recovery_context(), PIN)
        .await
        .expect("enabled");

    let decoded = RecoveryData::decode(&data).expect("recovery data");
    assert_eq!(decoded.account_hash, account_hash(ACCOUNT));
    assert!(matches!(
        recovery_state(&client).await.expect("state"),
        RecoveryState::Enabled { .. }
    ));
    assert_eq!(services.evaluations(), 1, "one attempt to seal");
}

#[tokio::test]
async fn a_second_enable_refuses() {
    let services = Services::start();
    let (browser, client, _) = enabled(&services).await;

    let again = enable_recovery(&client, &browser.kupferdraht.recovery_context(), PIN).await;

    assert!(
        matches!(again, Err(KupferdrahtError::InvalidInput(_))),
        "{again:?}"
    );
}

#[tokio::test]
async fn enable_refuses_a_pin_the_rules_refuse() {
    let services = Services::start();
    let browser = Browser::new(Some(&services));
    let client = browser.first_setup().await;

    let result = enable_recovery(&client, &browser.kupferdraht.recovery_context(), "123456").await;

    assert!(
        matches!(result, Err(KupferdrahtError::InvalidInput(_))),
        "{result:?}"
    );
    assert_eq!(services.evaluations(), 0, "no attempt spent");
}

#[tokio::test]
async fn without_a_pin_service_recovery_refuses() {
    let browser = Browser::new(None);
    let client = browser.first_setup().await;

    let result = enable_recovery(&client, &browser.kupferdraht.recovery_context(), PIN).await;

    assert!(
        matches!(result, Err(KupferdrahtError::InvalidInput(_))),
        "{result:?}"
    );
}

/// The PIN check is local: it spends no attempt at the PIN service.
#[tokio::test]
async fn verify_pin_checks_locally() {
    let services = Services::start();
    let (browser, client, _) = enabled(&services).await;
    let context = browser.kupferdraht.recovery_context();
    let attempts = services.evaluations();

    assert!(
        verify_recovery_pin(&client, &context, PIN)
            .await
            .expect("checked")
    );
    assert!(
        !verify_recovery_pin(&client, &context, "482914")
            .await
            .expect("checked")
    );
    assert_eq!(services.evaluations(), attempts, "no attempt spent");
}

#[tokio::test]
async fn change_pin_needs_the_current_pin() {
    let services = Services::start();
    let (browser, client, _) = enabled(&services).await;
    let context = browser.kupferdraht.recovery_context();

    let refused = change_recovery_pin(&client, &context, "482914", "735190").await;
    assert!(
        matches!(
            refused,
            Err(KupferdrahtError::WrongPin {
                attempts_remaining: None
            })
        ),
        "{refused:?}"
    );

    let data = change_recovery_pin(&client, &context, PIN, "735190")
        .await
        .expect("changed");
    assert!(RecoveryData::decode(&data).is_ok());
    assert!(
        verify_recovery_pin(&client, &context, "735190")
            .await
            .expect("checked")
    );
    assert!(
        !verify_recovery_pin(&client, &context, PIN)
            .await
            .expect("checked")
    );
}

/// The browser lost everything: a login there, the recovery data and the
/// PIN bring the account's seed back, and the device registers.
#[tokio::test]
async fn recover_in_a_new_browser_brings_the_seed_back() {
    let services = Services::start();
    let (browser_a, _client, data) = enabled(&services).await;
    let browser_b = Browser::new(Some(&services));
    let pending = browser_b.login().await;
    assert_eq!(pending.keys(), KeysState::Missing);

    let client = pending
        .recover(&data, PIN, NATIVE_PATH)
        .await
        .expect("recovered");

    assert_eq!(browser_b.seed(), browser_a.seed(), "the same seed");
    assert_eq!(
        client.user_id().map(|u| u.as_str().to_owned()).as_deref(),
        Some(ACCOUNT)
    );
}

/// Step 120 (ADR-0040): the slot names another device — this data is a
/// replaced device's; it is refused before the PIN service is asked.
#[tokio::test]
async fn a_replaced_devices_data_is_refused_without_an_attempt() {
    let services = Services::start();
    let (_browser_a, _client, data) = enabled(&services).await;
    let newer = Services::with_slot(r#"{"slot":{"label":"fc100a0123ef263d","claimedAtMs":2}}"#);
    let browser_b = Browser::new(Some(&newer));
    let pending = browser_b.login().await;

    let result = pending.recover(&data, PIN, NATIVE_PATH).await;

    match result {
        Err(KupferdrahtError::InvalidRecoveryData { reason }) => assert_eq!(reason, "oldDevice"),
        other => panic!("{:?}", other.err()),
    }
    assert_eq!(newer.evaluations(), 0, "no attempt spent");
    assert!(browser_b.seed().is_none(), "nothing stored");
}

/// Step 120 (ADR-0040): `recoveryExpected` is the slot's — a set-up device
/// has recovery data; no device, or one being set up, has none.
#[tokio::test]
async fn recovery_expected_follows_the_slot() {
    for (slot, expected) in [
        (
            format!(r#"{{"slot":{{"label":"{CLIENT}","claimedAtMs":1}}}}"#),
            true,
        ),
        (r#"{"slot":{"label":"","claimedAtMs":1}}"#.to_owned(), false),
        (r#"{"slot":null}"#.to_owned(), false),
    ] {
        let services = Services::with_slot(&slot);
        let browser = Browser::new(Some(&services));

        let pending = browser.login().await;

        assert_eq!(pending.recovery_expected(), expected, "{slot}");
    }
}

/// A wrong PIN stores nothing; the pending login stays usable — a first
/// setup works right after.
#[tokio::test]
async fn a_wrong_pin_stores_nothing_and_the_login_stays_usable() {
    let services = Services::start();
    let (_browser_a, _client, data) = enabled(&services).await;
    let browser_b = Browser::new(Some(&services));
    let pending = browser_b.login().await;

    let result = pending.recover(&data, "482914", NATIVE_PATH).await;

    assert!(
        matches!(
            result,
            Err(KupferdrahtError::WrongPin {
                attempts_remaining: Some(_)
            })
        ),
        "{:?}",
        result.err()
    );
    assert!(browser_b.seed().is_none(), "nothing stored");
    assert_eq!(pending.keys(), KeysState::Missing);
    pending
        .register_device_with(FIRST_SETUP)
        .await
        .expect("a first setup still works");
}

#[tokio::test]
async fn recover_with_the_seed_present_refuses() {
    let services = Services::start();
    let (browser, _client, data) = enabled(&services).await;
    let pending = browser.login().await;
    assert_eq!(pending.keys(), KeysState::Present);

    let result = pending.recover(&data, PIN, NATIVE_PATH).await;

    assert!(
        matches!(result, Err(KupferdrahtError::InvalidInput(_))),
        "{:?}",
        result.err()
    );
}

/// Another account's recovery data is refused before the PIN service is
/// asked.
#[tokio::test]
async fn another_accounts_data_is_refused() {
    let services = Services::start();
    let (_browser_a, _client, data) = enabled(&services).await;
    let mut foreign = RecoveryData::decode(&data).expect("recovery data");
    foreign.account_hash = account_hash("other-uuid@example.test");
    let browser_b = Browser::new(Some(&services));
    let pending = browser_b.login().await;
    let attempts = services.evaluations();

    let result = pending.recover(&foreign.encode(), PIN, NATIVE_PATH).await;

    match result {
        Err(KupferdrahtError::InvalidRecoveryData { reason }) => assert_eq!(reason, "otherAccount"),
        other => panic!("{:?}", other.err()),
    }
    assert_eq!(services.evaluations(), attempts, "no attempt spent");
    assert!(browser_b.seed().is_none());
}

/// A recovered seed is the account's: `cancel` forgets it in this browser,
/// but the databases the registration opened stay — on the storage server
/// they are those of every browser of the account.
#[tokio::test]
async fn cancel_after_a_recovery_keeps_the_accounts_databases() {
    let services = Services::start();
    let (_browser_a, _client, data) = enabled(&services).await;
    let browser_b = Browser::at_device_cap(&services);
    let pending = browser_b.login().await;

    let result = pending.recover(&data, PIN, NATIVE_PATH).await;

    assert!(
        matches!(result, Err(KupferdrahtError::TooManyDevices)),
        "{:?}",
        result.err()
    );
    assert_eq!(pending.keys(), KeysState::Present, "the seed was recovered");
    assert!(browser_b.account_dir().exists(), "the storage opened");
    pending.cancel().expect("cancel");
    assert!(
        browser_b.seed().is_none(),
        "the recovered seed is forgotten"
    );
    assert!(
        browser_b.account_dir().exists(),
        "the account's databases stay"
    );
}
