// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `LoginPipeline` — orchestrates the multi-step login flow.
//!
//! Decomposes what was previously a single 51-line `ClientBuilder::login`
//! body into discrete phases, each under the 40-line limit:
//!
//! 1. [`LoginPipeline::start`] — parse the server config, build the
//!    `HttpClient`, negotiate the Wire API version.
//! 2. [`LoginPipeline::authenticate`] — `POST /v{N}/login` + `GET /v{N}/self`.
//! 3. [`LoginPipeline::register_and_rebind`] — `POST /v{N}/clients`
//!    followed by the mandatory `POST /v{N}/access?client_id=<id>` so
//!    nginx injects the `Z-Client` claim on subsequent calls.
//! 4. [`LoginPipeline::upload_initial_key_packages`] — fresh MLS
//!    identity + 100 `KeyPackage`s → `POST /v{N}/mls/key-packages/self/{client_id}`.
//! 5. [`LoginPipeline::fetch_backend_removal_key`] —
//!    `GET /v{N}/mls/public-keys`, pick the ECDSA-P256 `removal` entry.
//! 6. [`LoginPipeline::finalize`] — assemble the `Account` + `Client`.
//!
//! No public symbols — the pipeline is an internal composition used by
//! `ClientBuilder::login`.

use crate::time::now_ms;
use std::sync::{Arc, Mutex as StdMutex};

use base64::Engine as _;
use openmls_sqlite_storage::Connection;
use tracing::{debug, warn};

use crate::client::{Account, Client, ServerConfig};
use crate::encryption::{
    INITIAL_KEY_PACKAGE_COUNT, KupferdrahtProvider, MLS_PUBLIC_KEY_TYPE, MlsClient,
};
use crate::error::KupferdrahtError;
use crate::identity::UserId;
use crate::storage::db_key::DbKey;
use crate::storage::{KupferdrahtHome, Store, UserPropertyEntry};
use crate::wire::HttpClient;
use crate::wire::dto::{LoginOutcome, SelfUser};
use crate::wire::select_api_version;

/// Outcome of [`LoginPipeline::register_and_rebind`] — the new device
/// id from `POST /v{N}/clients`, the rebound access token from
/// `POST /v{N}/access?client_id=<id>`, and the **rotated zuid** the
/// rebind response set. Wire rotates the cookie on every successful
/// /access; the caller must persist `refresh_token` from here, not
/// the original login zuid (live-confirmed).
pub(super) struct RegistrationOutcome {
    pub(super) device_id: String,
    pub(super) access_token: String,
    pub(super) refresh_token: String,
}

/// Whether a login can reuse the persisted device (Step 40). See
/// [`LoginPipeline::reuse_existing_device_if_valid`].
pub(super) enum DeviceReuse {
    /// The persisted device is still registered on Wire — reuse it + its MLS.
    Reused(RegistrationOutcome),
    /// A prior device existed locally but Wire no longer has it: register a new
    /// one and wipe the superseded MLS / history.
    Replaced,
    /// No prior local session — a clean first login.
    Fresh,
}

/// Outcome of [`LoginPipeline::upload_initial_key_packages`] — the
/// bits of MLS identity that outlive the local `MlsClient` and must be
/// persisted by the caller (e.g. the CLI into `devices.mls_public_key`),
/// plus the live [`MlsClient`] itself. From Step 3 onward the client
/// is retained on [`Client`] so `Client::join_room` and follow-up
/// send / receive paths can drive openmls without re-opening the
/// provider.
pub(super) struct MlsIdentityArtifacts {
    pub(super) signature_public_key: Vec<u8>,
    pub(super) mls_client: Arc<StdMutex<MlsClient>>,
}

/// Open a [`KupferdrahtProvider`]: file-backed at the account's `mls.db`
/// (`KupferdrahtHome::account_mls`) when a home was configured, in-memory
/// otherwise. On native it creates the account directory if it does not
/// exist; in the browser the path is a file name in the OPFS pool (Step
/// 91, ADR-0022) and there is no directory to create.
///
/// The single crate-internal seam through which the SDK obtains the MLS
/// store — the login pipeline and `resume` (`super::resume`) both route
/// through it on every target. See `docs/plan/archive/61-mls-storage-pluggable.md`.
pub(super) fn open_mls_provider(
    home: Option<&KupferdrahtHome>,
    account_id: &str,
    db_key: Option<&DbKey>,
) -> Result<KupferdrahtProvider, KupferdrahtError> {
    let Some(home) = home else {
        return KupferdrahtProvider::open_in_memory();
    };
    // Step 80: same materialization-site guard as
    // `storage::open_default_store` — the id is runtime data.
    if !crate::storage::is_path_safe_id(account_id) {
        return Err(KupferdrahtError::InvalidInput(format!(
            "open_mls_provider: refusing path-unsafe account id {account_id:?}"
        )));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let dir = home.account_dir(account_id);
        std::fs::create_dir_all(&dir)
            .map_err(|e| KupferdrahtError::Storage(format!("mkdir {}: {e}", dir.display())))?;
    }
    let path = home.account_mls(account_id);
    let conn = open_mls_connection(&path, db_key)?;
    let provider = KupferdrahtProvider::open(conn)?;
    // Step 113: mls.db checks its VFS failure like kupferdraht.db.
    #[cfg(target_arch = "wasm32")]
    let provider = {
        let mut provider = provider;
        provider.watch_database(path);
        provider
    };
    Ok(provider)
}

/// Open the `rusqlite` connection under `mls.db`'s own key (separate from
/// `kupferdraht.db`'s), keyed before openmls runs its refinery migrations. The
/// single place the `mls.db` connection is configured — the third of the
/// three open paths (Step 85: the multi-process pragmas live here too).
fn open_mls_connection(
    path: &std::path::Path,
    db_key: Option<&DbKey>,
) -> Result<Connection, KupferdrahtError> {
    let conn = crate::storage::open_connection(path)?;
    if let Some(key) = db_key {
        crate::storage::apply_whole_db_key(&conn, key)?;
    }
    // Step 85: busy_timeout + explicit synchronous=FULL (native). mls.db
    // keeps SQLite's rollback journal (no WAL here).
    #[cfg(not(target_arch = "wasm32"))]
    crate::storage::set_multi_process_pragmas(&conn)?;
    Ok(conn)
}

/// Stateful pipeline carrying the transport handle and the negotiated
/// API version across the authentication phases.
pub(super) struct LoginPipeline {
    http: HttpClient,
    server: ServerConfig,
    api_version: u32,
    home: Option<KupferdrahtHome>,
    store: Option<Arc<dyn Store>>,
    /// Per-install persistent-cookie label (Step 41). Wire keys the refresh
    /// cookie on `(user, label)`; a label unique to this install keeps two
    /// devices of the same account from invalidating each other's `zuid`. Used
    /// for both `POST /login` (the persist cookie) and `POST /clients` (the
    /// device's cookie slot).
    cookie_label: String,
    /// Human-readable device label shown in Wire's device list (Step 41), e.g.
    /// the OS device name from the embedder. `None` → a platform-derived default.
    device_label: Option<String>,
}

/// Cookie label used for both the Wire `cookie` field of the client
/// registration payload and the stable per-user label under which Wire
/// stores the device cookie. Includes the Wire user id so a single
/// account running multiple Kupferdraht installs stays distinguishable.
/// Read the persisted `devices.mls_public_key` for `(user_id, device_id)`
/// and base64-decode it. Used only by `resume_existing_mls_state`;
/// returns `InvalidInput` when the row or the column is missing,
/// `Encoding` for malformed base64.
async fn load_persisted_mls_public_key(
    store: &Arc<dyn Store>,
    user_id: &str,
    device_id: &str,
) -> Result<Vec<u8>, KupferdrahtError> {
    let device = store
        .load_device(user_id, device_id)
        .await?
        .ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "resume_existing_mls_state: no devices row for {user_id}/{device_id}"
            ))
        })?;
    let mls_public_key_b64 = device.mls_public_key.ok_or_else(|| {
        KupferdrahtError::InvalidInput(
            "resume_existing_mls_state: persisted device has no mls_public_key".into(),
        )
    })?;
    base64::engine::general_purpose::STANDARD
        .decode(&mls_public_key_b64)
        .map_err(|e| {
            KupferdrahtError::Encoding(format!(
                "resume_existing_mls_state: mls_public_key not base64: {e}"
            ))
        })
}

/// Map Wire's `400 too-many-clients` (returned by `register_client` when
/// the per-user device cap is reached) to the typed
/// [`KupferdrahtError::TooManyDevices`] so callers can offer device-limit
/// recovery; every other error passes through unchanged. Step 24.
fn map_too_many_clients(err: KupferdrahtError) -> KupferdrahtError {
    match &err {
        KupferdrahtError::BackendRejected(body) if body.contains("too-many-clients") => {
            KupferdrahtError::TooManyDevices
        }
        _ => err,
    }
}

impl LoginPipeline {
    /// Build the `HttpClient` and negotiate the API version against
    /// `GET /api-version`. `home`, when set, locates the per-account
    /// `mls.db` (`KupferdrahtHome::account_mls`) for file-backed openmls
    /// persistence (PLAN.md Step 1c.6). `permit_http` must be `false`
    /// for production callers; the flag is threaded from
    /// `ClientBuilder::permit_http` and only the `httpmock`-backed
    /// integration tests set it to `true`.
    pub(super) async fn start(
        server: ServerConfig,
        home: Option<KupferdrahtHome>,
        permit_http: bool,
        store: Option<Arc<dyn Store>>,
        cookie_label: String,
        device_label: Option<String>,
    ) -> Result<Self, KupferdrahtError> {
        let http = HttpClient::new(&server.api_base_url, permit_http)?;
        let api_info = http.api_version().await?;
        let api_version = select_api_version(&api_info.supported)?;
        Ok(Self {
            http,
            server,
            api_version,
            home,
            store,
            cookie_label,
            device_label,
        })
    }

    /// The Wire API version negotiated in [`Self::start`]. Read by
    /// `PendingLogin` for the post-auth device-list / device-delete
    /// calls it drives on its own transport.
    pub(super) fn api_version(&self) -> u32 {
        self.api_version
    }

    /// Bind the per-account storage after `authenticate` revealed the
    /// account id — the per-account `kupferdraht.db` / `mls.db` directory
    /// (`base/accounts/<id>/`) cannot be named before then, so the
    /// pipeline starts storage-less and the default store is opened and
    /// set here.
    pub(super) fn bind_storage(
        &mut self,
        store: Option<Arc<dyn Store>>,
        home: Option<KupferdrahtHome>,
    ) {
        self.store = store;
        self.home = home;
    }

    /// Authenticate and fetch the self-profile.
    pub(super) async fn authenticate(
        &self,
        email: &str,
        password: &str,
        second_factor_code: Option<&str>,
    ) -> Result<(LoginOutcome, SelfUser), KupferdrahtError> {
        let login_outcome = self
            .http
            .login(
                self.api_version,
                email,
                password,
                second_factor_code,
                &self.cookie_label,
            )
            .await?;
        let self_user = self
            .http
            .fetch_self(self.api_version, &login_outcome.access_token)
            .await?;
        Ok((login_outcome, self_user))
    }

    /// SSO analogue of [`authenticate`](Self::authenticate) (Step 17β): mint
    /// the first access token from the `zuid` cookie handed back by Wire's
    /// `/sso/finalize-login` redirect, then fetch the self-profile. No
    /// password and no `client_id` — the rest of the pipeline (phases 3–6)
    /// is identical to the password flow.
    pub(super) async fn finalize_sso(
        &self,
        sso_cookie: &str,
    ) -> Result<(LoginOutcome, SelfUser), KupferdrahtError> {
        let login_outcome = self
            .http
            .access_from_sso_cookie(self.api_version, sso_cookie)
            .await?;
        let self_user = self
            .http
            .fetch_self(self.api_version, &login_outcome.access_token)
            .await?;
        Ok((login_outcome, self_user))
    }

    /// Fresh MLS identity + 100 `KeyPackage`s → upload to
    /// `POST /v{N}/mls/key-packages/self/{client_id}`.
    ///
    /// Creates a fresh [`MlsClient`] for the `(user_id, device_id)`
    /// pair, generates [`INITIAL_KEY_PACKAGE_COUNT`] TLS-serialized
    /// `KeyPackage`s, and uploads them with the rebound access token.
    ///
    /// Private key material stays in the `MlsClient`'s in-memory
    /// `OpenMlsRustCrypto` provider which is dropped when this method
    /// returns — until 1c.5 swaps in an SQLite-backed provider, the
    /// uploaded `KeyPackage`s are effectively single-use (the next
    /// login generates fresh ones). That is the deliberate interim
    /// state; no messaging flow depends on it before 1c.5.
    pub(super) async fn upload_initial_key_packages(
        &self,
        self_user: &SelfUser,
        registration: &RegistrationOutcome,
        mls_key: Option<&DbKey>,
    ) -> Result<MlsIdentityArtifacts, KupferdrahtError> {
        let user_id = self_user.qualified_id.to_string();
        let provider = open_mls_provider(self.home.as_ref(), &user_id, mls_key)?;
        let mls_client =
            MlsClient::new_fresh_with_storage(provider, &user_id, &registration.device_id)?;
        // Wire rejects the KeyPackage upload with
        // `mls-protocol-error: "No key associated to the given identity
        // and signature scheme"` unless the signature public key has been
        // registered first via PUT /v14/clients/{client_id} (observations.md §19).
        self.http
            .update_client_mls_public_key(
                self.api_version,
                &registration.access_token,
                &registration.device_id,
                MLS_PUBLIC_KEY_TYPE,
                &mls_client.signature_public_key_b64(),
            )
            .await?;
        // Wire's v9+ ConsumableNotifications event stream is gated
        // by an opt-in capability on the device (observations.md §22 —
        // live-confirmed: WebSocket opens
        // but no frames arrive without this). Declare both
        // capabilities v9+ devices must ship to bootstrap against
        // a live Wire v14 backend (live-validated set).
        self.http
            .update_client_capabilities(
                self.api_version,
                &registration.access_token,
                &registration.device_id,
                &["consumable-notifications", "legalhold-implicit-consent"],
            )
            .await?;
        let packages = mls_client.generate_key_packages(INITIAL_KEY_PACKAGE_COUNT)?;
        self.http
            .upload_key_packages(
                self.api_version,
                &registration.access_token,
                &registration.device_id,
                &packages,
            )
            .await?;
        let signature_public_key = mls_client.signature_public_key().to_vec();
        Ok(MlsIdentityArtifacts {
            signature_public_key,
            mls_client: Arc::new(StdMutex::new(mls_client)),
        })
    }

    /// `GET /v{N}/mls/public-keys` — fetch Wire's backend external-sender
    /// public keys and pick the `removal` / `ecdsa_secp256r1_sha256`
    /// entry. Returns the raw (base64-decoded) key bytes.
    ///
    /// # Errors
    /// * `BackendRejected` / `Network` / `Encoding` from the underlying
    ///   HTTP call.
    /// * `Encoding` if the `ecdsa_secp256r1_sha256` entry is not valid
    ///   base64.
    pub(super) async fn fetch_backend_removal_key(
        &self,
        access_token: &str,
    ) -> Result<Vec<u8>, KupferdrahtError> {
        let keys = self
            .http
            .fetch_mls_backend_public_keys(self.api_version, access_token)
            .await?;
        let b64 = &keys.removal.ecdsa_secp256r1_sha256;
        let key = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| {
                KupferdrahtError::Encoding(format!(
                    "mls/public-keys removal.{MLS_PUBLIC_KEY_TYPE} is not valid base64: {e}"
                ))
            })?;
        // Step 95c: the key lands in every group's external_senders
        // extension — an empty one is a malformed response.
        if key.is_empty() {
            return Err(KupferdrahtError::Encoding(format!(
                "mls/public-keys removal.{MLS_PUBLIC_KEY_TYPE} is empty"
            )));
        }
        Ok(key)
    }

    /// Step 7e / Step 40: decide whether a login can reuse the persisted device.
    /// When a session for this user already lives in `self.store`, validate the
    /// persisted `device_id` against Wire via `GET /v{N}/clients/{id}` and, if it
    /// still exists, rebind the freshly-issued access token to it — skipping
    /// `POST /v{N}/clients`.
    ///
    /// Three outcomes (`DeviceReuse`):
    /// * `Reused` — the device is still registered; reuse it + its MLS state.
    /// * `Replaced` — a prior device was persisted but Wire 404s it (gone). The
    ///   caller registers a new device **and wipes** the superseded local MLS /
    ///   history (Step 40 Part B) so the new device re-joins cleanly.
    /// * `Fresh` — no prior local session (or no store / no `device_id`): a clean
    ///   first login, nothing to wipe.
    pub(super) async fn reuse_existing_device_if_valid(
        &self,
        login: &LoginOutcome,
        self_user: &SelfUser,
    ) -> Result<DeviceReuse, KupferdrahtError> {
        // Every target reuses: `open_mls_provider` reopens the account's
        // `mls.db` (a file on native, an OPFS pool file in the browser), so a
        // reused device resumes its MLS identity — one path (ADR-0022).
        let Some(store) = &self.store else {
            return Ok(DeviceReuse::Fresh);
        };
        // Session/device rows key on the **qualified** `<uuid>@<domain>` user id
        // (Step 19) — `persist_session` writes them via `UserId::as_str()`. Look
        // up by the qualified form (`self_user.qualified_id`), NOT the bare
        // `self_user.id`: the bare uuid never matches, so `load_session` returns
        // `None`, the reuse path bails, and **every re-login registers a fresh
        // device** (piling up devices + orphaning MLS state). Step 40.
        let user_id = self_user.qualified_id.to_string();
        let Some(session) = store.load_session(&user_id).await? else {
            return Ok(DeviceReuse::Fresh);
        };
        let device_id = session.device_id;
        if device_id.is_empty() {
            return Ok(DeviceReuse::Fresh);
        }
        let still_registered = self
            .http
            .client_exists(self.api_version, &login.access_token, &device_id)
            .await?;
        if !still_registered {
            // The prior device is gone from Wire — the new one supersedes it.
            return Ok(DeviceReuse::Replaced);
        }
        let rebound = self
            .http
            .rebind_access_token(self.api_version, &login.refresh_token, &device_id)
            .await?;
        Ok(DeviceReuse::Reused(RegistrationOutcome {
            device_id,
            access_token: rebound.access_token,
            refresh_token: rebound.refresh_token,
        }))
    }

    /// Wipe the superseded device's local MLS groups + messaging history so a
    /// newly-registered replacement device starts clean and re-joins every room
    /// fresh (Step 40 Part B). The MLS groups must go via `discard_group` —
    /// `join_all_rooms` keys on the MLS store, not the rooms table, so clearing
    /// rooms alone would leave it skipping the (stale) rooms. Best-effort per
    /// group; the table clear is transactional. The timeline/chats are dropped
    /// (a genuinely new device has no claim to the old device's decrypted
    /// history); the room list is re-fetched on the next `sync_metadata`.
    pub(super) async fn wipe_superseded_state(
        &self,
        mls_identity: &MlsIdentityArtifacts,
    ) -> Result<(), KupferdrahtError> {
        let Some(store) = &self.store else {
            return Ok(());
        };
        let rooms = store.load_rooms().await?;
        {
            let guard = crate::encryption::lock_mls(&mls_identity.mls_client);
            for (room, _members) in &rooms {
                let Some(group_id_b64) = &room.group_id else {
                    continue;
                };
                let Ok(group_id) = base64::engine::general_purpose::STANDARD.decode(group_id_b64)
                else {
                    continue;
                };
                let _ = guard.discard_group(&group_id);
            }
        }
        store.clear_messaging_state().await
    }

    /// Step 7e companion to [`Self::reuse_existing_device_if_valid`].
    /// Loads the persisted `MlsClient` (signature keypair lives in
    /// the file-backed openmls provider) instead of generating a
    /// fresh identity + uploading new `KeyPackage`s. The reused
    /// device's `signature_public_key` is read from
    /// `devices.mls_public_key` so the caller can echo it onto the
    /// resulting `Client`.
    ///
    /// # Errors
    /// * `KupferdrahtError::InvalidInput` if no `base_dir` is
    ///   configured (can't host `mls.db`), no `devices` row matches
    ///   the persisted `(user_id, device_id)`, or its `mls_public_key`
    ///   is absent / not base64.
    /// * `KupferdrahtError::Storage` for openmls or `SQLite` failures.
    pub(super) async fn resume_existing_mls_state(
        &self,
        self_user: &SelfUser,
        registration: &RegistrationOutcome,
        mls_key: Option<&DbKey>,
    ) -> Result<MlsIdentityArtifacts, KupferdrahtError> {
        let Some(home) = self.home.as_ref() else {
            return Err(KupferdrahtError::InvalidInput(
                "resume_existing_mls_state: ClientBuilder::base_dir was not set; \
                 reuse path requires file-backed mls.db"
                    .into(),
            ));
        };
        let Some(store) = self.store.as_ref() else {
            return Err(KupferdrahtError::InvalidInput(
                "resume_existing_mls_state: no Store attached — set ClientBuilder::base_dir".into(),
            ));
        };
        let qualified_user_id = self_user.qualified_id.to_string();
        // Step 40: the devices row keys on the qualified user id too — load the
        // persisted MLS public key by `qualified_user_id`, not bare `self_user.id`.
        let mls_public_key =
            load_persisted_mls_public_key(store, &qualified_user_id, &registration.device_id)
                .await?;
        let provider = open_mls_provider(Some(home), &qualified_user_id, mls_key)?;
        let mls_client = MlsClient::resume_with_storage(
            provider,
            &qualified_user_id,
            &registration.device_id,
            &mls_public_key,
        )?;
        Ok(MlsIdentityArtifacts {
            signature_public_key: mls_public_key,
            mls_client: Arc::new(StdMutex::new(mls_client)),
        })
    }

    /// Register this device (`POST /v{N}/clients`) and rebind the access
    /// token (`POST /v{N}/access?client_id=<id>`) so nginx injects the
    /// `Z-Client` claim on every subsequent client-scoped call.
    pub(super) async fn register_and_rebind(
        &self,
        login: &LoginOutcome,
        password: Option<&str>,
    ) -> Result<RegistrationOutcome, KupferdrahtError> {
        let prekeys = crate::encryption::prekeys::build_prekey_bundle();
        let register = self
            .http
            .register_client(
                self.api_version,
                &login.access_token,
                password,
                &prekeys,
                &self.cookie_label,
                self.device_label.as_deref(),
            )
            .await
            .map_err(map_too_many_clients)?;
        let rebound = self
            .http
            .rebind_access_token(self.api_version, &login.refresh_token, &register.id)
            .await?;
        Ok(RegistrationOutcome {
            device_id: register.id,
            access_token: rebound.access_token,
            refresh_token: rebound.refresh_token,
        })
    }

    /// Step 10c — fetch and persist Wire's server-administered
    /// configuration: `/v14/feature-configs` (whole-team policy) and
    /// `/v14/properties` (per-user settings). Driven by `access_token`
    /// from the device-rebound `RegistrationOutcome`, so callers run
    /// this AFTER `register_and_rebind` (or `reuse_existing_device_if_valid`)
    /// has produced a client-bound token.
    ///
    /// Error posture is **warn-and-continue** for HTTP failures per
    /// ADR-0012 §Login pipeline integration: feature configs are an
    /// enrichment, not a login blocker. A transient outage on
    /// `/feature-configs` lets login proceed; the next sync cycle (10d)
    /// or the next login refreshes the cached values. Storage failures
    /// during the persist step DO propagate — a broken `SQLite` is not
    /// recoverable inside this phase.
    ///
    /// When no [`SqliteStore`] is attached to the pipeline (e.g. the
    /// in-memory test path), this returns `Ok(())` after a debug log —
    /// there is nowhere to persist to.
    pub(super) async fn fetch_server_configs(
        &self,
        access_token: &str,
    ) -> Result<(), KupferdrahtError> {
        let Some(store) = self.store.as_ref() else {
            debug!("fetch_server_configs: no store attached, skipping");
            return Ok(());
        };
        self.fetch_and_persist_feature_configs(store, access_token)
            .await?;
        self.fetch_and_persist_user_properties(store, access_token)
            .await?;
        Ok(())
    }

    /// 10c phase 1: `/v14/feature-configs` → `replace_feature_configs`.
    /// HTTP failure logs at warn and leaves the table untouched (the
    /// previously cached snapshot, if any, is preserved). Storage
    /// failure propagates.
    async fn fetch_and_persist_feature_configs(
        &self,
        store: &Arc<dyn Store>,
        access_token: &str,
    ) -> Result<(), KupferdrahtError> {
        match self
            .http
            .fetch_feature_configs(self.api_version, access_token)
            .await
        {
            Ok(raw) => {
                store
                    .replace_feature_configs(&raw.into_entries(now_ms()))
                    .await?;
            }
            Err(e) => warn!(
                error = ?e,
                "/feature-configs fetch failed; preserving cached values"
            ),
        }
        Ok(())
    }

    /// 10c phase 2: `/v14/properties` → per-key
    /// `/v14/properties/{key}` → `replace_user_properties`. The keys
    /// list itself failing logs warn + skips the whole phase. A
    /// per-key value fetch failing logs warn + skips that one key;
    /// the rest are still persisted via a single replace-all
    /// transaction.
    async fn fetch_and_persist_user_properties(
        &self,
        store: &Arc<dyn Store>,
        access_token: &str,
    ) -> Result<(), KupferdrahtError> {
        let keys = match self
            .http
            .fetch_property_keys(self.api_version, access_token)
            .await
        {
            Ok(keys) => keys,
            Err(e) => {
                warn!(
                    error = ?e,
                    "/properties keys fetch failed; preserving cached values"
                );
                return Ok(());
            }
        };
        let mut entries = Vec::with_capacity(keys.len());
        for k in keys {
            match self
                .http
                .fetch_property_value(self.api_version, access_token, &k)
                .await
            {
                Ok(v) => entries.push(UserPropertyEntry {
                    key: k,
                    value: v,
                    last_synced_at_ms: now_ms(),
                }),
                Err(e) => warn!(
                    key = %k,
                    error = ?e,
                    "property value fetch failed; skipping"
                ),
            }
        }
        store.replace_user_properties(&entries).await?;
        Ok(())
    }

    /// Assemble the final `Client`. Consumes the pipeline to signal the
    /// end of the flow. The per-account `home` is echoed onto `Client` so
    /// a subsequent `ClientBuilder::resume` call (or any future rebuild)
    /// can find the `mls.db` / asset cache without re-deriving the layout.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finalize(
        self,
        self_user: SelfUser,
        login: LoginOutcome,
        registration: RegistrationOutcome,
        backend_removal_key: Vec<u8>,
        mls_identity: MlsIdentityArtifacts,
        home: Option<KupferdrahtHome>,
    ) -> Arc<Client> {
        let localpart = login.user;
        let user_id = UserId::new(&localpart, &self_user.qualified_id.domain);
        let account = Account::new(
            user_id.clone(),
            self_user.name,
            self_user.handle,
            self_user.email,
        );
        Client::new(
            user_id,
            registration.device_id,
            self.server,
            self.api_version,
            registration.access_token,
            registration.refresh_token,
            account,
            backend_removal_key,
            mls_identity.signature_public_key,
            self.http,
            self.store,
            Some(mls_identity.mls_client),
            home,
            Some(self.cookie_label),
        )
    }
}

#[cfg(test)]
mod step_80_guard_tests {
    use super::open_mls_provider;
    use crate::storage::KupferdrahtHome;

    /// Symmetric to the `open_default_store` integration pin: the
    /// second mkdir site refuses a path-unsafe account id and
    /// creates nothing.
    #[test]
    fn open_mls_provider_refuses_path_unsafe_account_ids() {
        let dir = tempfile::tempdir().expect("tempdir");
        let home = KupferdrahtHome::with_home_id(
            dir.path().to_path_buf(),
            "0123456789abcdef0123456789abcdef".to_owned(),
        );
        for bad in ["../../evil@example.test", "a/b@c", ".."] {
            let result = open_mls_provider(Some(&home), bad, None);
            assert!(result.is_err(), "must refuse account id {bad:?}");
        }
        assert!(
            std::fs::read_dir(dir.path())
                .expect("read tempdir")
                .next()
                .is_none(),
            "no directory may be created for a refused id"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::GET;
    use httpmock::MockServer;

    fn server_config(base_url: &str) -> ServerConfig {
        ServerConfig {
            api_base_url: base_url.to_string(),
            websocket_url: base_url.to_string(),
            title: "mock".to_string(),
        }
    }

    #[tokio::test]
    async fn start_negotiates_api_version_against_server() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/api-version");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"supported":[14],"development":[]}"#);
        });

        let pipeline = LoginPipeline::start(
            server_config(&server.base_url()),
            None,
            true,
            None,
            "kupferdraht-test".to_string(),
            None,
        )
        .await
        .expect("start should succeed when server advertises v14");

        assert_eq!(pipeline.api_version, 14);
        assert_eq!(pipeline.server.api_base_url, server.base_url());
    }

    #[tokio::test]
    async fn start_rejects_server_without_v14_support() {
        let server = MockServer::start();
        server.mock(|when, then| {
            when.method(GET).path("/api-version");
            then.status(200)
                .header("content-type", "application/json")
                .body(r#"{"supported":[5,6,7,8],"development":[]}"#);
        });

        let err = LoginPipeline::start(
            server_config(&server.base_url()),
            None,
            true,
            None,
            "kupferdraht-test".to_string(),
            None,
        )
        .await
        .err()
        .expect("no v14 overlap must fail");
        assert!(
            matches!(err, KupferdrahtError::Environment(_)),
            "expected Environment error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn start_rejects_unparseable_base_url() {
        let err = LoginPipeline::start(
            ServerConfig {
                api_base_url: "::not a url::".to_string(),
                websocket_url: String::new(),
                title: String::new(),
            },
            None,
            true,
            None,
            "kupferdraht-test".to_string(),
            None,
        )
        .await
        .err()
        .expect("malformed URL must fail");
        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );
    }

    #[test]
    fn too_many_clients_maps_to_typed_error() {
        let rejected = KupferdrahtError::BackendRejected(
            r#"register_client 400: {"code":400,"label":"too-many-clients","message":"x"}"#.into(),
        );
        assert!(matches!(
            map_too_many_clients(rejected),
            KupferdrahtError::TooManyDevices
        ));
    }

    #[test]
    fn other_backend_errors_pass_through_unchanged() {
        let other = KupferdrahtError::BackendRejected("some other 400".into());
        assert!(matches!(
            map_too_many_clients(other),
            KupferdrahtError::BackendRejected(_)
        ));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod mls_connection_tests {
    use super::open_mls_connection;
    use crate::storage::db_key::generate_db_key;

    /// Step 85 (open.md §69 residual): `mls.db` is the third open path —
    /// a second process's write waits (`busy_timeout`) and durability
    /// stays full (`synchronous = FULL`, read back as 2).
    #[test]
    fn mls_connection_carries_busy_timeout_and_full_synchronous() {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = generate_db_key();
        let conn = open_mls_connection(&dir.path().join("mls.db"), Some(&key)).expect("open");
        let busy: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .expect("busy_timeout");
        assert_eq!(busy, 5000, "busy_timeout in effect on mls.db");
        let synchronous: i64 = conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .expect("synchronous");
        assert_eq!(synchronous, 2, "synchronous = FULL on mls.db");
    }
}
