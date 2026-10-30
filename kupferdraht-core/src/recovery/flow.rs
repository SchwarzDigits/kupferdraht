// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Sealing and opening recovery data for an account (Step 112, ADR-0035),
//! against the token exchange (a PIN token, `audience: "pin"`) and the PIN
//! service (the OPRF evaluation). Another account's data is refused before
//! any request — it costs no attempt. A failed open is `WrongPin` with the
//! attempts the service says are left. Since Step 124 a seal and an open
//! with the right PIN give their attempt back with a receipt signed by the
//! seed's receipt key — best effort: a refused receipt leaves the attempt
//! counted and fails nothing.

use rand::{CryptoRng, RngCore};
use zeroize::Zeroizing;

use super::data::{RecoveryData, SEALED_LEN, account_hash, client_id_field, invalid};
use super::oprf::{self, Blinded};
use super::pin::normalize_pin;
use super::pin_service::{self, Evaluation};
use super::receipt::{REFUND_KEY_LEN, ReceiptKey};
use super::seal::{ARGON2_PARAMS, open_seed, recovery_key, seal_seed};
use crate::error::KupferdrahtError;
use crate::storage::remote::wire_session::{TokenExchange, WireSession, buy_pin_token};

/// Where the PIN token, the OPRF evaluation and the account's storage slot
/// come from.
#[derive(Clone)]
pub(crate) struct RecoveryServices {
    pub(crate) exchange: TokenExchange,
    /// The PIN service's base URL, from the embedder's configuration.
    pub(crate) pin_service_url: String,
    /// Step 120 (ADR-0040): the storage server's slot query
    /// (`https://…/v1/slot`), from the storage URL.
    pub(crate) slot_url: String,
}

/// The account's storage slot, asked with a PIN token bought with
/// `session` (Step 120, ADR-0040): `None` — no device.
///
/// # Errors
/// The token exchange's errors; `StorageAccessDenied` / `ServiceUnavailable`
/// from the slot query.
pub(crate) async fn account_slot(
    services: &RecoveryServices,
    session: &WireSession,
) -> Result<Option<crate::storage::remote::slot::Slot>, KupferdrahtError> {
    let pin_token = buy_pin_token(&services.exchange, session).await?;
    crate::storage::remote::slot::fetch_slot(
        &services.slot_url,
        services.exchange.permit_http,
        &pin_token,
    )
    .await
}

/// The device recovery data belongs to (Step 120, ADR-0040): the account
/// and its Wire client — the storage slot's label.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RecoveryDevice<'a> {
    pub(crate) account_id: &'a str,
    pub(crate) client_id: &'a str,
}

/// The largest Argon2id memory an opened file may ask for (KiB): a
/// changed header must not exhaust the browser's memory.
const MAX_ARGON2_M_KIB: u32 = 262_144;

/// Seal `seed` with `pin` for `device`: the bytes the embedder keeps.
/// `rng` draws the OPRF blind first, then the salt and the nonce.
///
/// # Errors
/// The token exchange's and the PIN service's errors (`TooManyAttempts`,
/// `ServiceUnavailable`, `Unauthorized`, …).
pub(crate) async fn seal_for_account(
    services: &RecoveryServices,
    session: &WireSession,
    device: RecoveryDevice<'_>,
    seed: &[u8; 32],
    pin: &str,
    rng: &mut (impl RngCore + CryptoRng),
) -> Result<Vec<u8>, KupferdrahtError> {
    let receipt = ReceiptKey::derive(seed, device.account_id)?;
    let refund_key = receipt.public_key();
    let pin = normalize_pin(pin);
    let blinded = oprf::blind(pin.as_bytes(), rng)?;
    let mut salt = [0u8; 16];
    rng.fill_bytes(&mut salt);
    let mut nonce = [0u8; 12];
    rng.fill_bytes(&mut nonce);
    let evaluated = evaluate(services, session, &blinded, &pin, None, &refund_key).await?;
    let key = recovery_key(&evaluated.output, pin.as_bytes(), &salt, ARGON2_PARAMS)?;
    let mut data = RecoveryData {
        key_version: evaluated.evaluation.key_version,
        argon2: ARGON2_PARAMS,
        salt,
        nonce,
        account_hash: account_hash(device.account_id),
        client_id: client_id_field(device.client_id)?,
        created_at_ms: u64::try_from(crate::time::now_ms()).unwrap_or_default(),
        refund_key,
        sealed: [0; SEALED_LEN],
    };
    data.sealed = seal_seed(seed, &key, &data.header(), &nonce)?;
    give_back(services, &evaluated, &receipt).await;
    Ok(data.encode())
}

/// Open `data` with `pin` for `device` — the slot's: the account's seed.
/// `rng` draws the OPRF blind.
///
/// # Errors
/// `InvalidRecoveryData` for bytes that are not recovery data, of another
/// format version or Argon2id cost (`unsupported`), of another account
/// (`otherAccount`) or of another device (`oldDevice`) — both before any
/// request — or whose receipt key is not its seed's (`notRecoveryData`);
/// `WrongPin` with the attempts left; the token exchange's and the PIN
/// service's errors.
pub(crate) async fn open_for_account(
    services: &RecoveryServices,
    session: &WireSession,
    device: RecoveryDevice<'_>,
    data: &[u8],
    pin: &str,
    rng: &mut (impl RngCore + CryptoRng),
) -> Result<Zeroizing<[u8; 32]>, KupferdrahtError> {
    let data = RecoveryData::decode(data)?;
    check_header(&data, device)?;
    let pin = normalize_pin(pin);
    let blinded = oprf::blind(pin.as_bytes(), rng)?;
    let version = Some(data.key_version);
    let evaluated = evaluate(services, session, &blinded, &pin, version, &data.refund_key).await?;
    let key = recovery_key(&evaluated.output, pin.as_bytes(), &data.salt, data.argon2)?;
    let seed = open_seed(&data.sealed, &key, &data.header(), &data.nonce).map_err(|e| match e {
        KupferdrahtError::WrongPin { .. } => KupferdrahtError::WrongPin {
            attempts_remaining: evaluated.evaluation.attempts_remaining,
        },
        other => other,
    })?;
    // The header is authenticated: a key other than the seed's means the
    // data was sealed for another receipt key — not this seed's data.
    let receipt = ReceiptKey::derive(&seed, device.account_id)?;
    if receipt.public_key() != data.refund_key {
        return Err(invalid("notRecoveryData"));
    }
    give_back(services, &evaluated, &receipt).await;
    Ok(seed)
}

/// What the header says, checked before any request: the account, the
/// device, an Argon2id cost a browser can give.
///
/// # Errors
/// `InvalidRecoveryData`: `otherAccount`, `oldDevice`, `unsupported`.
fn check_header(data: &RecoveryData, device: RecoveryDevice<'_>) -> Result<(), KupferdrahtError> {
    if data.account_hash != account_hash(device.account_id) {
        return Err(invalid("otherAccount"));
    }
    if data.client_id != client_id_field(device.client_id)? {
        return Err(invalid("oldDevice"));
    }
    if data.argon2.m_kib > MAX_ARGON2_M_KIB {
        return Err(invalid("unsupported"));
    }
    Ok(())
}

/// Step 124: give the evaluated attempt back with `receipt`'s signature.
/// Best effort — an expired attempt, say, merely stays counted.
async fn give_back(services: &RecoveryServices, evaluated: &Evaluated, receipt: &ReceiptKey) {
    let attempt_id = &evaluated.evaluation.attempt_id;
    let refunded = pin_service::refund(
        &services.pin_service_url,
        services.exchange.permit_http,
        &evaluated.pin_token,
        attempt_id,
        &receipt.sign(attempt_id),
    )
    .await;
    if let Err(e) = refunded {
        tracing::warn!(error = %e, "the PIN service keeps the attempt: its receipt failed");
    }
}

/// An evaluated attempt: the finalized OPRF output, the service's answer,
/// and the PIN token its receipt goes with.
struct Evaluated {
    output: Zeroizing<[u8; 32]>,
    evaluation: Evaluation,
    pin_token: Zeroizing<String>,
}

/// A PIN token, the service's evaluation of `blinded` for the data of
/// `refund_key`, and the finalized OPRF output.
async fn evaluate(
    services: &RecoveryServices,
    session: &WireSession,
    blinded: &Blinded,
    pin: &str,
    key_version: Option<u32>,
    refund_key: &[u8; REFUND_KEY_LEN],
) -> Result<Evaluated, KupferdrahtError> {
    let pin_token = buy_pin_token(&services.exchange, session).await?;
    let evaluation = pin_service::evaluate(
        &services.pin_service_url,
        services.exchange.permit_http,
        &pin_token,
        key_version,
        refund_key,
        blinded.element(),
    )
    .await?;
    let output = oprf::finalize(blinded, pin.as_bytes(), &evaluation.evaluated)?;
    Ok(Evaluated {
        output,
        evaluation,
        pin_token,
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use httpmock::Method::POST;
    use httpmock::{Mock, MockServer};
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use serde_json::json;

    use super::*;
    use crate::recovery::data::RecoveryData;
    use crate::recovery::oprf::{blind, evaluate_as_service};
    use crate::recovery::pin::normalize_pin;
    use crate::recovery::receipt::{REFUND_KEY_LEN, ReceiptKey};

    const ACCOUNT: &str = "user-uuid@example.test";
    const CLIENT: &str = "db8ad281d4aa66";
    const DEVICE: RecoveryDevice<'static> = RecoveryDevice {
        account_id: ACCOUNT,
        client_id: CLIENT,
    };
    const SEED: [u8; 32] = [7; 32];
    /// The mock PIN service's OPRF key.
    const SERVICE_KEY: [u8; 32] = [0x42; 32];
    /// The attempt id the mock service hands out.
    const ATTEMPT: &str = "AAECAwQFBgcICQoLDA0ODw==";

    /// The receipt key of `SEED` for `ACCOUNT`, compressed.
    fn refund_key() -> [u8; REFUND_KEY_LEN] {
        ReceiptKey::derive(&SEED, ACCOUNT)
            .expect("receipt key")
            .public_key()
    }

    fn services(exchange: &MockServer, pin_service: &MockServer) -> RecoveryServices {
        RecoveryServices {
            slot_url: format!("{}/v1/slot", exchange.base_url()),
            exchange: TokenExchange {
                url: exchange.base_url(),
                permit_http: true,
            },
            pin_service_url: pin_service.base_url(),
        }
    }

    fn pin_token(exchange: &MockServer) -> Mock<'_> {
        exchange.mock(|when, then| {
            when.method(POST)
                .path("/v1/token")
                .header("authorization", "Bearer wire.token")
                .json_body(json!({"audience": "pin"}));
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"token":"pin.jwt","expiresIn":600}"#);
        })
    }

    /// The mock service's answer to the blind the flow will draw from
    /// `seed` (the flow draws its blind first).
    fn evaluation<'a>(
        pin_service: &'a MockServer,
        pin: &str,
        seed: u64,
        key_version: Option<u32>,
        answer_version: u32,
        attempts_remaining: u32,
    ) -> Mock<'a> {
        evaluation_for(
            pin_service,
            pin,
            seed,
            key_version,
            &refund_key(),
            (answer_version, attempts_remaining),
        )
    }

    /// [`evaluation`] for data whose header names `refund_key`.
    fn evaluation_for<'a>(
        pin_service: &'a MockServer,
        pin: &str,
        seed: u64,
        key_version: Option<u32>,
        refund_key: &[u8; REFUND_KEY_LEN],
        (answer_version, attempts_remaining): (u32, u32),
    ) -> Mock<'a> {
        let blinded = blind(
            normalize_pin(pin).as_bytes(),
            &mut StdRng::seed_from_u64(seed),
        )
        .expect("blind");
        let evaluated = evaluate_as_service(&SERVICE_KEY, blinded.element());
        let mut body = json!({
            "refundKey": STANDARD.encode(refund_key),
            "blindedElement": STANDARD.encode(blinded.element())
        });
        if let Some(version) = key_version {
            body["keyVersion"] = json!(version);
        }
        pin_service.mock(|when, then| {
            when.method(POST)
                .path("/v1/evaluate")
                .header("authorization", "Bearer pin.jwt")
                .json_body(body);
            then.status(200)
                .header("content-type", "application/json")
                .body(
                    json!({
                        "keyVersion": answer_version,
                        "evaluatedElement": STANDARD.encode(evaluated),
                        "attemptsRemaining": attempts_remaining,
                        "attemptId": ATTEMPT
                    })
                    .to_string(),
                );
        })
    }

    async fn sealed(exchange: &MockServer, pin_service: &MockServer) -> Vec<u8> {
        evaluation(pin_service, "482913", 1, None, 7, 11);
        seal_for_account(
            &services(exchange, pin_service),
            &WireSession::Login(Zeroizing::new("wire.token".into())),
            DEVICE,
            &SEED,
            "482913",
            &mut StdRng::seed_from_u64(1),
        )
        .await
        .expect("sealed")
    }

    #[tokio::test]
    async fn sealed_recovery_data_opens_with_the_pin() {
        let (exchange, pin_service) = (MockServer::start(), MockServer::start());
        pin_token(&exchange);
        let data = sealed(&exchange, &pin_service).await;
        evaluation(&pin_service, "482913", 2, Some(7), 7, 10);

        let seed = open_for_account(
            &services(&exchange, &pin_service),
            &WireSession::Login(Zeroizing::new("wire.token".into())),
            DEVICE,
            &data,
            "482913",
            &mut StdRng::seed_from_u64(2),
        )
        .await
        .expect("opened");

        assert_eq!(*seed, SEED);
    }

    /// The service's key version goes into the file and back on opening.
    #[tokio::test]
    async fn the_key_version_travels_in_the_header() {
        let (exchange, pin_service) = (MockServer::start(), MockServer::start());
        pin_token(&exchange);

        let data = sealed(&exchange, &pin_service).await;

        let decoded = RecoveryData::decode(&data).expect("recovery data");
        assert_eq!(decoded.key_version, 7);
        assert_eq!(
            decoded.account_hash,
            crate::recovery::data::account_hash(ACCOUNT)
        );
    }

    #[tokio::test]
    async fn a_wrong_pin_says_how_many_attempts_are_left() {
        let (exchange, pin_service) = (MockServer::start(), MockServer::start());
        pin_token(&exchange);
        let data = sealed(&exchange, &pin_service).await;
        evaluation(&pin_service, "482914", 3, Some(7), 7, 3);

        let result = open_for_account(
            &services(&exchange, &pin_service),
            &WireSession::Login(Zeroizing::new("wire.token".into())),
            DEVICE,
            &data,
            "482914",
            &mut StdRng::seed_from_u64(3),
        )
        .await;

        assert!(
            matches!(
                result,
                Err(KupferdrahtError::WrongPin {
                    attempts_remaining: Some(3)
                })
            ),
            "{:?}",
            result.err()
        );
    }

    /// A file whose header asks for more Argon2id memory than a browser
    /// should give is refused before any request.
    #[tokio::test]
    async fn an_excessive_argon2_cost_is_unsupported() {
        let (exchange, pin_service) = (MockServer::start(), MockServer::start());
        let token = pin_token(&exchange);
        let mut data = RecoveryData::decode(&sealed(&exchange, &pin_service).await).expect("data");
        data.argon2.m_kib = 4 * 1024 * 1024;
        let calls_after_sealing = token.calls();

        let result = open_for_account(
            &services(&exchange, &pin_service),
            &WireSession::Login(Zeroizing::new("wire.token".into())),
            DEVICE,
            &data.encode(),
            "482913",
            &mut StdRng::seed_from_u64(5),
        )
        .await;

        match result {
            Err(KupferdrahtError::InvalidRecoveryData { reason }) => {
                assert_eq!(reason, "unsupported");
            }
            other => panic!("{:?}", other.err()),
        }
        assert_eq!(token.calls(), calls_after_sealing, "no PIN token asked for");
    }

    /// Another account's file is refused before the PIN service is asked:
    /// it costs no attempt.
    /// Step 120 (ADR-0040): data of another device — the slot holds a
    /// newer one — is refused before the PIN service is asked.
    #[tokio::test]
    async fn another_devices_data_is_refused_without_a_request() {
        let (exchange, pin_service) = (MockServer::start(), MockServer::start());
        let token = pin_token(&exchange);
        let data = sealed(&exchange, &pin_service).await;
        let calls_after_sealing = token.calls();

        let result = open_for_account(
            &services(&exchange, &pin_service),
            &WireSession::Login(Zeroizing::new("wire.token".into())),
            RecoveryDevice {
                account_id: ACCOUNT,
                client_id: "fc100a0123ef263d",
            },
            &data,
            "482913",
            &mut StdRng::seed_from_u64(6),
        )
        .await;

        match result {
            Err(KupferdrahtError::InvalidRecoveryData { reason }) => {
                assert_eq!(reason, "oldDevice");
            }
            other => panic!("{:?}", other.err()),
        }
        assert_eq!(token.calls(), calls_after_sealing, "no PIN token asked for");
    }

    /// The device the data was sealed for goes into the header.
    #[tokio::test]
    async fn the_device_travels_in_the_header() {
        let (exchange, pin_service) = (MockServer::start(), MockServer::start());
        pin_token(&exchange);

        let data = RecoveryData::decode(&sealed(&exchange, &pin_service).await).expect("data");

        assert_eq!(
            crate::recovery::data::client_id_text(&data.client_id),
            CLIENT
        );
        assert!(data.created_at_ms > 1_700_000_000_000, "the creation time");
    }

    #[tokio::test]
    async fn another_accounts_data_is_refused_without_a_request() {
        let (exchange, pin_service) = (MockServer::start(), MockServer::start());
        let token = pin_token(&exchange);
        let data = sealed(&exchange, &pin_service).await;
        let calls_after_sealing = token.calls();

        let result = open_for_account(
            &services(&exchange, &pin_service),
            &WireSession::Login(Zeroizing::new("wire.token".into())),
            RecoveryDevice {
                account_id: "other-uuid@example.test",
                client_id: CLIENT,
            },
            &data,
            "482913",
            &mut StdRng::seed_from_u64(4),
        )
        .await;

        match result {
            Err(KupferdrahtError::InvalidRecoveryData { reason }) => {
                assert_eq!(reason, "otherAccount");
            }
            other => panic!("{:?}", other.err()),
        }
        assert_eq!(token.calls(), calls_after_sealing, "no PIN token asked for");
    }

    /// Step 124: `POST /v1/refund` accepting only a receipt for `ATTEMPT`
    /// whose signature verifies with `refund_key` — checked independently of
    /// the code under test.
    fn receipts(pin_service: &MockServer, refund_key: [u8; REFUND_KEY_LEN]) -> Mock<'_> {
        use p256::ecdsa::signature::Verifier as _;
        use p256::ecdsa::{Signature, VerifyingKey};

        let message = format!("natrium-pin-refund-v1|example.test|user-uuid|{ATTEMPT}");
        pin_service.mock(move |when, then| {
            when.method(POST)
                .path("/v1/refund")
                .header("authorization", "Bearer pin.jwt")
                .is_true(move |req| {
                    let Ok(body) = serde_json::from_slice::<serde_json::Value>(req.body_ref())
                    else {
                        return false;
                    };
                    let signature = body["signature"]
                        .as_str()
                        .and_then(|s| STANDARD.decode(s).ok())
                        .and_then(|bytes| Signature::from_slice(&bytes).ok());
                    let key = VerifyingKey::from_sec1_bytes(&refund_key).expect("a P-256 point");
                    body["attemptId"] == ATTEMPT
                        && signature.is_some_and(|sig| key.verify(message.as_bytes(), &sig).is_ok())
                });
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"attemptsRemaining":12}"#);
        })
    }

    async fn open(
        exchange: &MockServer,
        pin_service: &MockServer,
        data: &[u8],
        pin: &str,
        rng_seed: u64,
    ) -> Result<Zeroizing<[u8; 32]>, KupferdrahtError> {
        open_for_account(
            &services(exchange, pin_service),
            &WireSession::Login(Zeroizing::new("wire.token".into())),
            DEVICE,
            data,
            pin,
            &mut StdRng::seed_from_u64(rng_seed),
        )
        .await
    }

    /// Step 124: the header carries the seed's receipt key — what the
    /// service keys its evaluation for this data by.
    #[tokio::test]
    async fn the_refund_key_travels_in_the_header() {
        let (exchange, pin_service) = (MockServer::start(), MockServer::start());
        pin_token(&exchange);

        let data = RecoveryData::decode(&sealed(&exchange, &pin_service).await).expect("data");

        assert_eq!(data.refund_key, refund_key());
    }

    /// An export costs no attempt: its receipt follows the evaluation.
    #[tokio::test]
    async fn sealing_gives_its_attempt_back() {
        let (exchange, pin_service) = (MockServer::start(), MockServer::start());
        pin_token(&exchange);
        let receipt = receipts(&pin_service, refund_key());

        sealed(&exchange, &pin_service).await;

        receipt.assert_calls(1);
    }

    /// A restore with the right PIN costs no attempt.
    #[tokio::test]
    async fn opening_with_the_right_pin_gives_its_attempt_back() {
        let (exchange, pin_service) = (MockServer::start(), MockServer::start());
        pin_token(&exchange);
        let data = sealed(&exchange, &pin_service).await;
        let receipt = receipts(&pin_service, refund_key());
        evaluation(&pin_service, "482913", 2, Some(7), 7, 10);

        let seed = open(&exchange, &pin_service, &data, "482913", 2)
            .await
            .expect("opened");

        assert_eq!(*seed, SEED);
        receipt.assert_calls(1);
    }

    /// A wrong PIN yields no seed, so no receipt: the attempt stays counted.
    #[tokio::test]
    async fn a_wrong_pin_gives_nothing_back() {
        let (exchange, pin_service) = (MockServer::start(), MockServer::start());
        pin_token(&exchange);
        let data = sealed(&exchange, &pin_service).await;
        let receipt = receipts(&pin_service, refund_key());
        evaluation(&pin_service, "482914", 3, Some(7), 7, 3);

        let result = open(&exchange, &pin_service, &data, "482914", 3).await;

        assert!(matches!(result, Err(KupferdrahtError::WrongPin { .. })));
        receipt.assert_calls(0);
    }

    /// A receipt the service refuses — expired, say — fails nothing: the
    /// attempt merely stays counted.
    #[tokio::test]
    async fn a_refused_receipt_fails_nothing() {
        let (exchange, pin_service) = (MockServer::start(), MockServer::start());
        pin_token(&exchange);
        pin_service.mock(|when, then| {
            when.method(POST).path("/v1/refund");
            then.status(404)
                .header("content-type", "application/json")
                .body(r#"{"error":"unknown_attempt"}"#);
        });
        let data = sealed(&exchange, &pin_service).await;
        evaluation(&pin_service, "482913", 2, Some(7), 7, 10);

        let seed = open(&exchange, &pin_service, &data, "482913", 2)
            .await
            .expect("opened despite the refused receipt");

        assert_eq!(*seed, SEED);
    }

    /// Data that opens but whose header names another receipt key than its
    /// seed's is not this seed's data — refused, and no receipt is sent.
    #[tokio::test]
    async fn a_refund_key_that_is_not_the_seeds_is_refused() {
        let (exchange, pin_service) = (MockServer::start(), MockServer::start());
        pin_token(&exchange);
        let foreign = ReceiptKey::derive(&[9; 32], ACCOUNT)
            .expect("receipt key")
            .public_key();
        let data = sealed_with_refund_key(foreign);
        let receipt = receipts(&pin_service, foreign);
        evaluation_for(&pin_service, "482913", 2, Some(7), &foreign, (7, 10));

        let result = open(&exchange, &pin_service, &data, "482913", 2).await;

        match result {
            Err(KupferdrahtError::InvalidRecoveryData { reason }) => {
                assert_eq!(reason, "notRecoveryData");
            }
            other => panic!("{:?}", other.err()),
        }
        receipt.assert_calls(0);
    }

    /// `SEED` sealed with PIN "482913" as the mock service would, but with
    /// `refund_key` in the header.
    fn sealed_with_refund_key(refund_key: [u8; REFUND_KEY_LEN]) -> Vec<u8> {
        use crate::recovery::seal::{ARGON2_PARAMS, recovery_key, seal_seed};

        let pin = normalize_pin("482913");
        let blinded = blind(pin.as_bytes(), &mut StdRng::seed_from_u64(1)).expect("blind");
        let evaluated = evaluate_as_service(&SERVICE_KEY, blinded.element());
        let output = oprf::finalize(&blinded, pin.as_bytes(), &evaluated).expect("finalize");
        let mut data = RecoveryData {
            key_version: 7,
            argon2: ARGON2_PARAMS,
            salt: [0x11; 16],
            nonce: [0x22; 12],
            account_hash: crate::recovery::data::account_hash(ACCOUNT),
            client_id: crate::recovery::data::client_id_field(CLIENT).expect("client id"),
            created_at_ms: 1_791_000_000_000,
            refund_key,
            sealed: [0; SEALED_LEN],
        };
        let key = recovery_key(&output, pin.as_bytes(), &data.salt, ARGON2_PARAMS).expect("key");
        data.sealed = seal_seed(&SEED, &key, &data.header(), &data.nonce).expect("sealed");
        data.encode()
    }
}
