// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

use thiserror::Error;

#[derive(Debug, Error, uniffi::Error)]
pub enum KupferdrahtError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("second factor required")]
    SecondFactorRequired,
    #[error("invalid second factor code")]
    InvalidSecondFactor,
    #[error("not logged in")]
    NotLoggedIn,
    /// HTTP 401 from an authenticated Wire call — the access token is
    /// expired or rejected. Distinct from `BackendRejected` so the
    /// in-session refresh seam can match it and retry once with a fresh
    /// token.
    #[error("access token expired or rejected")]
    Unauthorized,
    /// The refresh cookie (`zuid`) is no longer valid — `POST /access`
    /// returned `403` with label `invalid-credentials` ("Invalid zauth
    /// token"). **Terminal**: unlike `Unauthorized` (a 401 the refresh seam
    /// recovers), a dead `zuid` cannot be refreshed and the user must
    /// re-authenticate. Distinct from `BackendRejected` so the sync loop can
    /// stop retrying and signal a re-login instead of looping. The MLS state +
    /// chats survive (re-login reuses the device); only the auth tokens are
    /// cleared. Step 39.
    #[error("session expired; the refresh token is no longer valid — re-login required")]
    SessionExpired,
    #[error("network error: {0}")]
    Network(String),
    #[error("backend rejected the request: {0}")]
    BackendRejected(String),
    #[error("encoding error: {0}")]
    Encoding(String),
    /// Cryptographic integrity check failed — a hash, MAC, or
    /// authenticated-encryption tag did not match. Distinct from
    /// `Encoding` because integrity failures are semantically a
    /// "tampering / corruption" signal, not a "malformed input"
    /// signal. Used by Step 11's asset receive path to reject a
    /// ciphertext blob whose SHA-256 disagrees with the value
    /// carried in the MLS-protected `Asset.RemoteData.sha256`.
    #[error("integrity check failed: {0}")]
    IntegrityCheck(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("environment error: {0}")]
    Environment(String),
    /// A `Client::sync*` method was invoked while another sync
    /// iteration is already in flight on the same `Client`. Step
    /// 14b guarantees at most one cursor advance at a time across
    /// the four `sync_once` / `sync` / `sync_with_callback` /
    /// `sync_stream` entry points — concurrent callers must
    /// serialise themselves.
    #[error("a sync iteration is already in flight on this Client")]
    SyncAlreadyRunning,
    /// The operation needs a loaded MLS group, but the local openmls
    /// store has no group for this room — typically because the
    /// caller has Wire metadata for the room (it shows up in the
    /// cache after `sync_metadata`) but never ran `Client::join_room`
    /// to bring up the local MLS state. Distinct from `InvalidInput`
    /// so callers can match the specific case and recover (e.g.
    /// `Timeline::send_text_auto_join` calls `join_room` and
    /// retries). Step 14g.
    #[error("MLS group not loaded for this conversation; call `Client::join_room` first")]
    MlsGroupMissing,
    /// A send targeted a stale MLS epoch — the conversation advanced (a commit
    /// the client has not processed yet) and Wire rejected the message with
    /// `409 mls-stale-message`. Distinct from `BackendRejected` so the send
    /// path can match it, resync the room to the current epoch, and retry.
    /// Step 37.
    #[error("MLS message epoch is stale; resync the conversation and retry")]
    MlsStaleMessage,
    /// An **inbound** MLS message could not be processed because its epoch
    /// differs from the local group's (openmls `ValidationError::WrongEpoch`):
    /// a commit advancing the group is still pending / was missed. Recoverable
    /// in-order — distinct from `Generic` so the receive drain can hold the
    /// cursor + retry, and escalate to a resync only on a persistent gap.
    /// Step 38.
    #[error(
        "inbound MLS message is out of order (epoch mismatch); the room resyncs to the \
         current epoch — the message itself is not re-processed"
    )]
    MlsOutOfOrder,
    /// Authentication succeeded, but registering a new device hit Wire's
    /// per-user device cap (`POST /clients` → `400 too-many-clients`).
    /// Surfaced from `PendingLogin::register_device`; distinct from
    /// `BackendRejected` so callers can match it and recover on the same
    /// `PendingLogin` — list devices, delete one, call `register_device`
    /// again — without re-authenticating.
    #[error("device limit reached — too many registered devices for this account")]
    TooManyDevices,
    /// Another Kupferdraht process holds the advisory lock this operation
    /// needs (Step 85): the per-account lock around the token
    /// refresh + persist and around the first-time creation of an
    /// account's database keys, or the per-home lock around the
    /// first-time creation of the device-registry key. Fail-fast and
    /// **retryable** — the holder finishes its critical section in
    /// milliseconds and the caller simply tries again; nothing was
    /// changed on this side. The payload names the locked scope and
    /// the holder's pid when the lock file recorded one.
    #[error("locked by another Kupferdraht process: {0}")]
    AccountLocked(String),
    /// The database exists but the keystore returned no key for it
    /// (ADR-0029, Step 96). Nothing was written to the keystore: a
    /// locked or not-yet-ready keystore (iOS before the first unlock)
    /// succeeds on a later attempt; a key that is gone for good leaves
    /// the database unreadable. The payload names the database, the
    /// keystore id and the way out — for `global.db` the home's
    /// registry file, for `kupferdraht.db` / `mls.db` the account directory.
    #[error("the key for an existing database is unavailable: {0}")]
    KeyUnavailable(String),
    /// A home whose account keys come from the account's seed (the
    /// browser, ADR-0035) holds no seed for this account: none of its keys
    /// can be made, and a login opens nothing of it. Recoverable — restore
    /// the account's recovery data, or set up this browser as a new device
    /// (`register_device` with a first setup). Native homes never raise it.
    /// The payload names the account.
    #[error("recovery data required: this browser holds no keys for {0}")]
    RecoveryDataRequired(String),
    /// Browser only (ADR-0035): another instance holds the database — the
    /// open was refused with `SQLITE_BUSY`. Take it over on request, or
    /// close the other one. The payload names the database. Step 111a.
    #[error("the database is held by another instance: {0}")]
    HeldElsewhere(String),
    /// Browser only (ADR-0035): the storage quota is exhausted
    /// (`SQLITE_FULL`); the commit was rolled back. The payload names the
    /// database. Step 111a.
    #[error("the browser's storage is full: {0}")]
    StorageFull(String),
    /// Browser only (ADR-0023, ADR-0035): this instance was stopped —
    /// another tab took it over — and every further store operation fails.
    /// Terminate the worker. Step 111a.
    #[error("this Kupferdraht instance was evicted; terminate its worker")]
    Evicted,
    /// Browser only (ADR-0035): the storage server holds no database for
    /// the account — `resume` opened without creating and the server
    /// deleted it (retention). Set the browser up again. Step 111b.
    #[error("nothing stored for {0}")]
    NothingStored(String),
    /// Browser only (ADR-0036): the storage refused this account — the token
    /// exchange's admission does not admit the user, or the storage server
    /// rejected the token. Step 111b.
    #[error("no storage access: {0}")]
    StorageAccessDenied(String),
    /// Browser only (ADR-0035): the storage server or the token exchange is
    /// unreachable or overloaded; try again later. Step 111b.
    #[error("storage service unavailable: {0}")]
    ServiceUnavailable(String),
    /// Browser only (ADR-0035): the PIN does not open the recovery data.
    /// `attempts_remaining` is how many more attempts the PIN service
    /// allows before its limit, when it said so. Step 112.
    #[error("wrong PIN")]
    WrongPin { attempts_remaining: Option<u32> },
    /// Browser only (ADR-0035): the PIN service's attempt limit is reached;
    /// try again after `retry_after_secs`. Step 112.
    #[error("too many PIN attempts")]
    TooManyAttempts { retry_after_secs: Option<u64> },
    /// Browser only (ADR-0035): these bytes cannot be opened as recovery
    /// data — `reason` is `notRecoveryData`, `unsupported` (an unknown
    /// format version, or a key version the PIN service retired) or
    /// `otherAccount`; since Step 120 also `oldDevice` (the data belongs to
    /// a device another one replaced). Step 112.
    #[error("invalid recovery data: {reason}")]
    InvalidRecoveryData { reason: String },
    /// Browser only (ADR-0040): another device of the user holds the
    /// account's storage slot — this browser's keys belong to a device it
    /// replaced (`SQLITE_PERM`). The browser forgot them; log in again and
    /// recover or set up. The payload names the database. Step 120.
    #[error("this device was replaced by another one: {0}")]
    DeviceRetired(String),
    /// The backend rate-limited this client (`429`). `retry_after_secs`
    /// carries its `Retry-After` in seconds when it sent one. Distinct from
    /// `BackendRejected` so the sync loops can wait it out and carry on
    /// instead of aborting. Step 103.
    #[error("rate-limited by the backend; retry later")]
    RateLimited { retry_after_secs: Option<u64> },
    #[error("unexpected error: {0}")]
    Generic(String),
}

impl KupferdrahtError {
    /// The variant's name — the `name` of the error the browser binding
    /// throws, so JavaScript can branch on it as Kotlin and Swift do on the
    /// typed error (ADR-0035). Step 110.
    #[cfg(any(target_arch = "wasm32", test))]
    pub(crate) fn variant_name(&self) -> &'static str {
        match self {
            Self::InvalidCredentials => "InvalidCredentials",
            Self::SecondFactorRequired => "SecondFactorRequired",
            Self::InvalidSecondFactor => "InvalidSecondFactor",
            Self::NotLoggedIn => "NotLoggedIn",
            Self::Unauthorized => "Unauthorized",
            Self::SessionExpired => "SessionExpired",
            Self::Network(_) => "Network",
            Self::BackendRejected(_) => "BackendRejected",
            Self::Encoding(_) => "Encoding",
            Self::IntegrityCheck(_) => "IntegrityCheck",
            Self::InvalidInput(_) => "InvalidInput",
            Self::Storage(_) => "Storage",
            Self::Environment(_) => "Environment",
            Self::SyncAlreadyRunning => "SyncAlreadyRunning",
            Self::MlsGroupMissing => "MlsGroupMissing",
            Self::MlsStaleMessage => "MlsStaleMessage",
            Self::MlsOutOfOrder => "MlsOutOfOrder",
            Self::TooManyDevices => "TooManyDevices",
            Self::AccountLocked(_) => "AccountLocked",
            Self::KeyUnavailable(_) => "KeyUnavailable",
            Self::RecoveryDataRequired(_) => "RecoveryDataRequired",
            Self::HeldElsewhere(_) => "HeldElsewhere",
            Self::StorageFull(_) => "StorageFull",
            Self::Evicted => "Evicted",
            Self::NothingStored(_) => "NothingStored",
            Self::StorageAccessDenied(_) => "StorageAccessDenied",
            Self::ServiceUnavailable(_) => "ServiceUnavailable",
            Self::WrongPin { .. } => "WrongPin",
            Self::TooManyAttempts { .. } => "TooManyAttempts",
            Self::InvalidRecoveryData { .. } => "InvalidRecoveryData",
            Self::DeviceRetired(_) => "DeviceRetired",
            Self::RateLimited { .. } => "RateLimited",
            Self::Generic(_) => "Generic",
        }
    }
}

#[cfg(test)]
mod variant_name_tests {
    use super::KupferdrahtError;

    /// Every variant, one value each — a new variant fails to compile in
    /// `variant_name` until it is named there.
    fn one_of_each() -> Vec<KupferdrahtError> {
        vec![
            KupferdrahtError::InvalidCredentials,
            KupferdrahtError::SecondFactorRequired,
            KupferdrahtError::InvalidSecondFactor,
            KupferdrahtError::NotLoggedIn,
            KupferdrahtError::Unauthorized,
            KupferdrahtError::SessionExpired,
            KupferdrahtError::Network(String::new()),
            KupferdrahtError::BackendRejected(String::new()),
            KupferdrahtError::Encoding(String::new()),
            KupferdrahtError::IntegrityCheck(String::new()),
            KupferdrahtError::InvalidInput(String::new()),
            KupferdrahtError::Storage(String::new()),
            KupferdrahtError::Environment(String::new()),
            KupferdrahtError::SyncAlreadyRunning,
            KupferdrahtError::MlsGroupMissing,
            KupferdrahtError::MlsStaleMessage,
            KupferdrahtError::MlsOutOfOrder,
            KupferdrahtError::TooManyDevices,
            KupferdrahtError::AccountLocked(String::new()),
            KupferdrahtError::KeyUnavailable(String::new()),
            KupferdrahtError::RecoveryDataRequired(String::new()),
            KupferdrahtError::HeldElsewhere(String::new()),
            KupferdrahtError::StorageFull(String::new()),
            KupferdrahtError::Evicted,
            KupferdrahtError::NothingStored(String::new()),
            KupferdrahtError::StorageAccessDenied(String::new()),
            KupferdrahtError::ServiceUnavailable(String::new()),
            KupferdrahtError::WrongPin {
                attempts_remaining: None,
            },
            KupferdrahtError::TooManyAttempts {
                retry_after_secs: None,
            },
            KupferdrahtError::InvalidRecoveryData {
                reason: String::new(),
            },
            KupferdrahtError::DeviceRetired(String::new()),
            KupferdrahtError::RateLimited {
                retry_after_secs: None,
            },
            KupferdrahtError::Generic(String::new()),
        ]
    }

    /// The name is the variant, as `Debug` spells it.
    #[test]
    fn every_error_is_named_after_its_variant() {
        for error in one_of_each() {
            let debug = format!("{error:?}");
            let variant = debug.split(['(', ' ', '{']).next().expect("a variant name");
            assert_eq!(error.variant_name(), variant, "{debug}");
        }
    }

    #[test]
    fn the_browser_outcomes_have_their_names() {
        assert_eq!(
            KupferdrahtError::RecoveryDataRequired("a@b".into()).variant_name(),
            "RecoveryDataRequired"
        );
        assert_eq!(
            KupferdrahtError::SessionExpired.variant_name(),
            "SessionExpired"
        );
        assert_eq!(
            KupferdrahtError::TooManyDevices.variant_name(),
            "TooManyDevices"
        );
    }
}
