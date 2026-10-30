// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! An account's recovery (Step 112, ADR-0035) — what `Client.recovery()`
//! offers: the state, the first recovery data (`enable`), new data under a
//! new PIN (`changePin`, the current PIN checked locally) and the local PIN
//! check (`verifyPin`). The seed never leaves Kupferdraht; only the sealed bytes
//! do. The state lives in `kupferdraht.db`.

use std::sync::{Arc, Weak};

use rand::rngs::OsRng;

use super::flow::{RecoveryDevice, RecoveryServices, seal_for_account};
use super::pin::{PinCheck, validate_new_pin};
use super::verifier::{pin_matches, pin_verifier};
use crate::client::Client;
use crate::error::KupferdrahtError;
use crate::keystore::SecureKeyStore;
use crate::storage::account_keys::{SEED_LEN, load_seed};
use crate::storage::remote::wire_session::WireSession;
use crate::storage::{RecoveryStateRecord, Store};
use crate::wire::http::TokenRefresh;

/// What an account's recovery calls need besides the client.
#[derive(Clone)]
pub(crate) struct RecoveryContext {
    /// Where the account's seed is kept.
    pub(crate) keystore: Arc<dyn SecureKeyStore>,
    pub(crate) home_id: String,
    /// The token exchange and the PIN service; `None` without a PIN
    /// service (the local mode, or none configured).
    pub(crate) services: Option<RecoveryServices>,
}

/// Whether the account has recovery data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RecoveryState {
    Disabled,
    Enabled { exported_at_ms: i64 },
}

/// The account's recovery state.
///
/// # Errors
/// Storage errors.
pub(crate) async fn recovery_state(client: &Client) -> Result<RecoveryState, KupferdrahtError> {
    let Some(store) = client.store() else {
        return Ok(RecoveryState::Disabled);
    };
    Ok(match store.load_recovery_state().await? {
        Some(record) => RecoveryState::Enabled {
            exported_at_ms: record.exported_at_ms,
        },
        None => RecoveryState::Disabled,
    })
}

/// The first recovery data: the account's seed sealed with `pin`. Only
/// while disabled.
///
/// # Errors
/// `InvalidInput` when enabled already, without a PIN service, or for a
/// PIN `validateNewPin` refuses; the sealing's errors.
pub(crate) async fn enable_recovery(
    client: &Arc<Client>,
    context: &RecoveryContext,
    pin: &str,
) -> Result<Vec<u8>, KupferdrahtError> {
    let services = services_of(context)?;
    require_new_pin(pin)?;
    let store = store_of(client)?;
    if store.load_recovery_state().await?.is_some() {
        return Err(KupferdrahtError::InvalidInput(
            "recovery is enabled already: change the PIN instead".into(),
        ));
    }
    seal_and_record(client, context, services, store.as_ref(), pin).await
}

/// New recovery data under `new_pin`; `current_pin` is checked locally.
///
/// # Errors
/// `WrongPin` (no attempts: nothing was asked) for another current PIN;
/// `InvalidInput` while disabled, without a PIN service, or for a new PIN
/// `validateNewPin` refuses; the sealing's errors.
pub(crate) async fn change_recovery_pin(
    client: &Arc<Client>,
    context: &RecoveryContext,
    current_pin: &str,
    new_pin: &str,
) -> Result<Vec<u8>, KupferdrahtError> {
    let services = services_of(context)?;
    if !verify_recovery_pin(client, context, current_pin).await? {
        return Err(KupferdrahtError::WrongPin {
            attempts_remaining: None,
        });
    }
    require_new_pin(new_pin)?;
    let store = store_of(client)?;
    seal_and_record(client, context, services, store.as_ref(), new_pin).await
}

/// Whether `pin` is the account's recovery PIN — local, no attempt spent.
///
/// # Errors
/// `InvalidInput` while disabled; storage and keystore errors.
pub(crate) async fn verify_recovery_pin(
    client: &Client,
    context: &RecoveryContext,
    pin: &str,
) -> Result<bool, KupferdrahtError> {
    let Some(record) = store_of(client)?.load_recovery_state().await? else {
        return Err(KupferdrahtError::InvalidInput(
            "recovery is not enabled".into(),
        ));
    };
    let seed = account_seed(client, context)?;
    pin_matches(&seed, pin, &record.pin_verifier)
}

/// Seal the account's seed with `pin` and record the state: the time and
/// the PIN's verifier.
async fn seal_and_record(
    client: &Arc<Client>,
    context: &RecoveryContext,
    services: &RecoveryServices,
    store: &dyn Store,
    pin: &str,
) -> Result<Vec<u8>, KupferdrahtError> {
    let seed = account_seed(client, context)?;
    let refresher: Weak<dyn TokenRefresh> = Arc::downgrade(client) as Weak<Client>;
    let session = WireSession::Client(refresher);
    let account_id = client_account_id(client);
    // Step 120 (ADR-0040): the data belongs to this device — the slot's.
    let client_id = client.device_id().ok_or_else(|| {
        KupferdrahtError::InvalidInput("recovery needs a registered device".into())
    })?;
    let device = RecoveryDevice {
        account_id: &account_id,
        client_id,
    };
    let data = seal_for_account(services, &session, device, &seed, pin, &mut OsRng).await?;
    let record = RecoveryStateRecord {
        exported_at_ms: crate::time::now_ms(),
        pin_verifier: pin_verifier(&seed, pin)?,
    };
    store.save_recovery_state(&record).await?;
    debug_assert!(!data.is_empty(), "sealed recovery data");
    Ok(data)
}

/// The recovery services, or `InvalidInput` without a PIN service.
fn services_of(context: &RecoveryContext) -> Result<&RecoveryServices, KupferdrahtError> {
    context.services.as_ref().ok_or_else(|| {
        KupferdrahtError::InvalidInput(
            "recovery needs a PIN service (storage.pinServiceUrl)".into(),
        )
    })
}

/// `InvalidInput` for a PIN `validateNewPin` refuses.
fn require_new_pin(pin: &str) -> Result<(), KupferdrahtError> {
    match validate_new_pin(pin) {
        PinCheck::Ok => Ok(()),
        refused => Err(KupferdrahtError::InvalidInput(format!(
            "the PIN is {}",
            refused.as_str()
        ))),
    }
}

/// The client's store, or `InvalidInput` for a session without one.
fn store_of(client: &Client) -> Result<Arc<dyn Store>, KupferdrahtError> {
    client
        .store()
        .ok_or_else(|| KupferdrahtError::InvalidInput("recovery needs the account's store".into()))
}

/// The account's seed, or `InvalidInput` in a home without one (native).
fn account_seed(
    client: &Client,
    context: &RecoveryContext,
) -> Result<zeroize::Zeroizing<[u8; SEED_LEN]>, KupferdrahtError> {
    let account_id = client_account_id(client);
    load_seed(&context.keystore, &context.home_id, &account_id)?.ok_or_else(|| {
        KupferdrahtError::InvalidInput("this home holds no seed for the account".into())
    })
}

/// The client's qualified account id.
fn client_account_id(client: &Client) -> String {
    client.account().user_id().as_str().to_owned()
}
