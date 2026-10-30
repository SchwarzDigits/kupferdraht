// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `ClientBuilder::resume` orchestration helpers.
//!
//! `ClientBuilder::resume` reconstructs a `Client` from a persisted
//! `SqliteStore` — no Wire round-trip, just deserialise the on-disk
//! state and reopen the openmls provider. The actual orchestration
//! lives in [`super::builder`]; this file holds the per-step helpers
//! it composes (load-from-store, access-token freshness check,
//! account / server projection, MLS resume).
//!
//! Every helper here is `pub(super)` — only [`super::builder`]
//! consumes them.

use crate::time::now_ms;
use std::sync::{Arc, Mutex as StdMutex};

use base64::Engine as _;

use crate::client::Account;
use crate::client::Client;
use crate::client::builder::ServerConfig;
use crate::client::login_pipeline::{
    DeviceReuse, LoginPipeline, MlsIdentityArtifacts, RegistrationOutcome,
};
use crate::encryption::MlsClient;
use crate::error::KupferdrahtError;
use crate::identity::UserId;
use crate::storage::db_key::DbKey;
use crate::storage::{KupferdrahtHome, Store};
use crate::wire::HttpClient;
use crate::wire::dto::{LoginOutcome, SelfUser};

/// Wire access tokens have a ~15-minute TTL. Past this slightly tighter
/// threshold `resume` refreshes the token up front via
/// [`refresh_stale_access_token`] rather than risk a mid-command `401`;
/// the margin leaves a CLI command headroom to finish on the fresh
/// token.
const ACCESS_TOKEN_MAX_AGE_MS: i64 = 14 * 60 * 1000;

/// Step 7e: prefer reusing an existing device row when the store
/// already carries one for this user. Falls through to a fresh
/// `POST /v{N}/clients` only when there is no store, no prior
/// session, an empty `device_id` in the session row (legacy pre-7e
/// data), or Wire reports the persisted client is gone (`404`).
///
/// Step 10c: between the device-rebound access token landing
/// ([`LoginPipeline::register_and_rebind`] /
/// [`LoginPipeline::reuse_existing_device_if_valid`]) and the MLS
/// identity step, the pipeline calls
/// [`LoginPipeline::fetch_server_configs`] to seed the V18
/// `feature_configs` and `user_properties` tables. Errors there are
/// warn-and-continue per ADR-0012 — feature-configs are an
/// enrichment, not a login blocker.
pub(super) async fn establish_device_identity(
    pipeline: &LoginPipeline,
    login_outcome: &LoginOutcome,
    self_user: &SelfUser,
    password: Option<&str>,
    mls_key: Option<&DbKey>,
) -> Result<DeviceIdentity, KupferdrahtError> {
    let decision = pipeline
        .reuse_existing_device_if_valid(login_outcome, self_user)
        .await?;
    // Captured before the `if let` moves the `Reused` payload out of `decision`.
    let is_replaced = matches!(decision, DeviceReuse::Replaced);
    if let DeviceReuse::Reused(registration) = decision {
        pipeline
            .fetch_server_configs(&registration.access_token)
            .await?;
        let mls_identity = pipeline
            .resume_existing_mls_state(self_user, &registration, mls_key)
            .await?;
        return Ok(DeviceIdentity {
            registration,
            mls_identity,
            replaced: false,
        });
    }
    let registration = pipeline
        .register_and_rebind(login_outcome, password)
        .await?;
    pipeline
        .fetch_server_configs(&registration.access_token)
        .await?;
    let mls_identity = pipeline
        .upload_initial_key_packages(self_user, &registration, mls_key)
        .await?;
    if is_replaced {
        // A prior device for this account was gone from Wire: this new device
        // supersedes it. Drop the old device's MLS groups + local history so the
        // new identity re-joins every room cleanly (Step 40 Part B).
        pipeline.wipe_superseded_state(&mls_identity).await?;
    }
    Ok(DeviceIdentity {
        registration,
        mls_identity,
        replaced: is_replaced,
    })
}

/// The device a login settled on ([`establish_device_identity`]): its
/// registration and MLS identity, and whether it replaced the device
/// recorded for the account because the backend no longer had it.
pub(super) struct DeviceIdentity {
    pub(super) registration: RegistrationOutcome,
    pub(super) mls_identity: MlsIdentityArtifacts,
    pub(super) replaced: bool,
}

/// When the persisted access token has aged past its ~15-min TTL,
/// refresh it in place: rebind via cookie-auth `POST /access` with the
/// persisted `zuid`, swap the fresh token + rotated `zuid` into
/// `session`, and re-persist. A live `zuid` keeps the session alive
/// across process restarts (no re-login); a dead `zuid` surfaces the
/// rebind error so the caller re-runs `login`.
///
/// Step 85: the same cross-process critical section as the in-session
/// refresh (`token_refresh` module docs). With a `home` the account's
/// process lock is taken (fail-fast: `AccountLocked`), and the session
/// is **re-read under the lock** — a fresh row another process persisted
/// between our load and now is adopted instead of rebound.
///
/// Supersedes the historical hard cap, which refused outright on
/// finding §24's (now live-disproven) assumption that a cross-process
/// rebind is rejected by Wire prod.
///
/// # Errors
/// * `Storage` for an empty `client_id`, `SessionExpired` for a blanked
///   refresh token (Step 95c — see `ensure_session_resumable`).
/// * `AccountLocked`, and whatever the rebind or the save surfaces.
pub(super) async fn refresh_stale_access_token(
    session: &mut crate::storage::SessionRecord,
    http: &HttpClient,
    store: &dyn Store,
    client_id: &str,
    home: Option<&KupferdrahtHome>,
) -> Result<(), KupferdrahtError> {
    ensure_session_resumable(session, client_id)?;
    if !access_token_is_stale(session) {
        return Ok(());
    }
    // wasm has no process lock — one instance per origin (ADR-0023).
    #[cfg(not(target_arch = "wasm32"))]
    let _process_lock = super::token_refresh::acquire_account_lock(home, &session.user_id)?;
    #[cfg(target_arch = "wasm32")]
    let _ = home;
    if let Some(fresh) = fresher_persisted_session(store, session).await? {
        tracing::debug!("resume: adopting the session another process refreshed");
        *session = fresh;
        return Ok(());
    }
    tracing::debug!(
        age_min = (now_ms() - session.access_token_issued_at_ms) / 60_000,
        "resume: access token aged out — rebinding via /access with the persisted zuid"
    );
    let rebound = http
        .rebind_access_token(session.api_version, &session.refresh_token, client_id)
        .await?;
    session.access_token = rebound.access_token;
    session.refresh_token = rebound.refresh_token;
    session.access_token_issued_at_ms = now_ms();
    store.save_session(session).await
}

/// Step 95c: the persisted rows are runtime data, never asserted. An
/// empty `client_id` is a damaged device row; an empty refresh token
/// (native) is the state `clear_persisted_tokens` leaves after
/// `SessionExpired` — the re-login gate. wasm is exempt from the second
/// check: there the empty string is the deliberate sentinel (the browser
/// jar owns the cookie — W6).
///
/// # Errors
/// `Storage` for an empty `client_id`; `SessionExpired` for an empty
/// native refresh token.
fn ensure_session_resumable(
    session: &crate::storage::SessionRecord,
    client_id: &str,
) -> Result<(), KupferdrahtError> {
    if client_id.is_empty() || session.device_id.is_empty() {
        return Err(KupferdrahtError::Storage(
            "resume: the persisted device has an empty client id — re-run login".into(),
        ));
    }
    #[cfg(not(target_arch = "wasm32"))]
    if session.refresh_token.is_empty() {
        return Err(KupferdrahtError::SessionExpired);
    }
    #[cfg(target_arch = "wasm32")]
    let _ = session;
    Ok(())
}

/// Stale when older than the token lifetime — or empty (Step 95c:
/// `clear_persisted_tokens` blanks it, and an empty bearer must never
/// reach a request).
fn access_token_is_stale(session: &crate::storage::SessionRecord) -> bool {
    session.access_token.is_empty()
        || now_ms() - session.access_token_issued_at_ms > ACCESS_TOKEN_MAX_AGE_MS
}

/// Re-read the persisted session under the lock (Step 85): another
/// process may have refreshed it since `session` was loaded. `Some` only
/// for a row that is fresh and carries non-empty tokens.
async fn fresher_persisted_session(
    store: &dyn Store,
    session: &crate::storage::SessionRecord,
) -> Result<Option<crate::storage::SessionRecord>, KupferdrahtError> {
    let persisted = store.load_session(&session.user_id).await?;
    Ok(persisted.filter(|fresh| {
        !access_token_is_stale(fresh)
            && !fresh.access_token.is_empty()
            && !fresh.refresh_token.is_empty()
    }))
}

/// Project the persisted `ServerConfigRecord` into the runtime
/// `ServerConfig`. Trivial mapping today; parked behind a helper so
/// the resume orchestration stays readable.
pub(super) fn build_server_from_record(rec: crate::storage::ServerConfigRecord) -> ServerConfig {
    ServerConfig {
        api_base_url: rec.api_base_url,
        websocket_url: rec.websocket_url,
        title: rec.title,
    }
}

/// Build the runtime `Client` from the persisted parts a successful
/// `load_resume_state` produced. Splits the long `Client::new` arg
/// list out of `resume` so the caller stays under the 40-line cap.
#[allow(clippy::too_many_arguments)]
pub(super) fn assemble_resumed_client(
    parts: ResumeState,
    server: ServerConfig,
    http: HttpClient,
    store: Arc<dyn Store>,
    mls_client: Option<Arc<StdMutex<MlsClient>>>,
    mls_public_key: Vec<u8>,
    kupferdraht_home: Option<KupferdrahtHome>,
) -> Result<Arc<Client>, KupferdrahtError> {
    let account = build_resume_account(&parts.self_user)?;
    let user_id = account.user_id().clone();
    Ok(Client::new(
        user_id,
        parts.device.client_id,
        server,
        parts.session.api_version,
        parts.session.access_token,
        parts.session.refresh_token,
        account,
        parts.removal_key.public_key,
        mls_public_key,
        http,
        Some(store),
        mls_client,
        kupferdraht_home,
        parts.session.cookie_label,
    ))
}

/// Build the [`Account`] view used by `resume` from the persisted
/// `SelfUserRecord` (whose `id` is the qualified user id since Step 19).
fn build_resume_account(
    self_user: &crate::storage::SelfUserRecord,
) -> Result<Arc<Account>, KupferdrahtError> {
    let user_id = UserId::parse(&self_user.id)?;
    Ok(Account::new(
        user_id,
        self_user.name.clone(),
        self_user.handle.clone(),
        self_user.email.clone(),
    ))
}

/// Reload the MLS provider for [`super::builder::ClientBuilder::resume`]
/// when a `base_dir` was configured. Without it (metadata-only
/// session) returns `Ok(None)` so `join_room` / `sync_members`
/// later surface a clear `InvalidInput`.
pub(super) fn resume_mls_state(
    home: Option<&KupferdrahtHome>,
    qualified_user: &str,
    client_id: &str,
    mls_public_key: &[u8],
    mls_key: Option<&DbKey>,
) -> Result<Option<Arc<StdMutex<MlsClient>>>, KupferdrahtError> {
    let Some(home) = home else {
        return Ok(None);
    };
    Ok(Some(resume_mls_client(
        home,
        qualified_user,
        client_id,
        mls_public_key,
        mls_key,
    )?))
}

/// Bundle of all `Store`-backed records that
/// [`super::builder::ClientBuilder::resume`] needs to reconstruct a
/// `Client`. Loading them up-front in one helper keeps `resume` itself
/// focused on the orchestration.
pub(super) struct ResumeState {
    pub(super) session: crate::storage::SessionRecord,
    pub(super) server_cfg: crate::storage::ServerConfigRecord,
    pub(super) device: crate::storage::DeviceRecord,
    pub(super) self_user: crate::storage::SelfUserRecord,
    pub(super) removal_key: crate::storage::BackendMlsKey,
}

pub(super) async fn load_resume_state(
    store: &dyn Store,
    account_id: &str,
) -> Result<ResumeState, KupferdrahtError> {
    let session = store.load_session(account_id).await?.ok_or_else(|| {
        KupferdrahtError::InvalidInput(format!(
            "no persisted session for {account_id} — run `kupferdraht-cli login` first"
        ))
    })?;
    let server_cfg = store.load_server_config().await?.ok_or_else(|| {
        KupferdrahtError::InvalidInput(
            "no persisted server config — run `kupferdraht-cli login` first".into(),
        )
    })?;
    let device = load_resume_device(store, &session).await?;
    let self_user = store
        .load_self_user(&session.user_id)
        .await?
        .ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "no persisted self_user for {}",
                session.user_id
            ))
        })?;
    // Step 95c: an empty persisted key (writable by pre-95c release
    // builds) counts as missing — it would reach `create_group`.
    let removal_key = store
        .load_backend_mls_key("removal", "ecdsa_secp256r1_sha256")
        .await?
        .filter(|key| !key.public_key.is_empty())
        .ok_or_else(|| {
            KupferdrahtError::InvalidInput(
                "no persisted backend removal key — re-run `login`".into(),
            )
        })?;
    Ok(ResumeState {
        session,
        server_cfg,
        device,
        self_user,
        removal_key,
    })
}

async fn load_resume_device(
    store: &dyn Store,
    session: &crate::storage::SessionRecord,
) -> Result<crate::storage::DeviceRecord, KupferdrahtError> {
    store
        .load_device(&session.user_id, &session.device_id)
        .await?
        .ok_or_else(|| {
            KupferdrahtError::InvalidInput(format!(
                "no persisted device {}/{} — re-run `kupferdraht-cli login`",
                session.user_id, session.device_id
            ))
        })
}

pub(super) fn decode_persisted_mls_public_key(
    b64: Option<&str>,
) -> Result<Vec<u8>, KupferdrahtError> {
    let s = b64.filter(|s| !s.is_empty()).ok_or_else(|| {
        KupferdrahtError::InvalidInput(
            "persisted device carries no MLS public key — re-run `login`".into(),
        )
    })?;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| KupferdrahtError::Encoding(format!("device.mls_public_key: {e}")))
}

/// Re-open the file-backed `MlsClient` for an existing device. Used by
/// [`super::builder::ClientBuilder::resume`]; keeps the openmls
/// provider warm across CLI invocations (and, since Step 91, across
/// browser reloads — the same `mls.db` lives in the OPFS pool).
fn resume_mls_client(
    home: &KupferdrahtHome,
    account_id: &str,
    client_id: &str,
    mls_public_key: &[u8],
    mls_key: Option<&DbKey>,
) -> Result<Arc<StdMutex<MlsClient>>, KupferdrahtError> {
    // Route through the shared crate-internal MLS-open seam (Step 43), the
    // same one the login pipeline uses, instead of a second inline open.
    let provider = super::login_pipeline::open_mls_provider(Some(home), account_id, mls_key)?;
    let mls = MlsClient::resume_with_storage(provider, account_id, client_id, mls_public_key)?;
    Ok(Arc::new(StdMutex::new(mls)))
}

#[cfg(test)]
mod refresh_stale_access_token_tests {
    use super::*;
    use crate::storage::SessionRecord;
    use crate::storage::SqliteStore;
    use crate::storage::db_key::DbKey;
    use httpmock::Method::POST;
    use httpmock::MockServer;
    use tempfile::TempDir;

    const CLIENT_ID: &str = "device-cid";

    /// A session whose access token is epoch-old — i.e. always past the
    /// TTL, so the rebind path is exercised.
    fn stale_session() -> SessionRecord {
        SessionRecord {
            user_id: "user-uuid".to_string(),
            device_id: CLIENT_ID.to_string(),
            access_token: "old.jwt.token".to_string(),
            refresh_token: "zuid-A".to_string(),
            cookie_label: Some("kupferdraht-user-uuid".to_string()),
            api_version: 14,
            access_token_issued_at_ms: 0,
        }
    }

    /// A whole-DB-keyed `SqliteStore` so `save_session` can persist the
    /// session. Returns the `TempDir` so the caller keeps the backing
    /// file alive for the test's duration.
    fn keyed_store() -> (TempDir, Arc<SqliteStore>) {
        let dir = tempfile::tempdir().expect("tempdir");
        let key = DbKey::from_keystore_bytes(&[0x42; 32]).expect("32-byte test key");
        let store = Arc::new(
            SqliteStore::open(&dir.path().join("kupferdraht.db"), &key).expect("open store"),
        );
        (dir, store)
    }

    fn http(base_url: &str) -> HttpClient {
        HttpClient::new(base_url, true).expect("HttpClient (permit_http=true)")
    }

    /// Mount the happy `/access` rebind: cookie `zuid=zuid-A` →
    /// 200 with a fresh access token and a rotated `zuid=zuid-B`.
    fn mount_rebind(server: &MockServer) -> httpmock::Mock<'_> {
        server.mock(|when, then| {
            when.method(POST)
                .path("/v14/access")
                .query_param("client_id", CLIENT_ID)
                .header("cookie", "zuid=zuid-A");
            then.status(200)
                .header("content-type", "application/json")
                .header("set-cookie", "zuid=zuid-B; HttpOnly; Secure; Path=/access")
                .body(
                    r#"{"access_token":"new.jwt.token","token_type":"Bearer","expires_in":900,"user":"user-uuid"}"#,
                );
        })
    }

    /// Stale token → cross-process rebind succeeds → fresh token +
    /// rotated zuid land in `session` AND are re-persisted.
    #[tokio::test]
    async fn rebinds_and_persists_when_token_is_stale() {
        let server = MockServer::start();
        let rebind = mount_rebind(&server);
        let (_dir, store) = keyed_store();
        let mut session = stale_session();

        refresh_stale_access_token(
            &mut session,
            &http(&server.base_url()),
            &*store,
            CLIENT_ID,
            None,
        )
        .await
        .expect("a stale token must refresh via /access rebind, not error");

        rebind.assert_calls(1);
        assert_eq!(
            session.access_token, "new.jwt.token",
            "in-memory token must be rebound"
        );
        assert_eq!(
            session.refresh_token, "zuid-B",
            "rotated zuid must be captured"
        );
        assert!(
            session.access_token_issued_at_ms > 0,
            "issued_at must be re-stamped"
        );

        let persisted = store
            .load_session("user-uuid")
            .await
            .expect("load_session")
            .expect("a session row must exist after refresh");
        assert_eq!(
            persisted.access_token, "new.jwt.token",
            "rebound token must be persisted"
        );
        assert_eq!(
            persisted.refresh_token, "zuid-B",
            "rotated zuid must be persisted"
        );
    }

    /// Fresh token → no `/access` call, session untouched.
    #[tokio::test]
    async fn is_noop_when_token_is_fresh() {
        let server = MockServer::start();
        let rebind = mount_rebind(&server);
        let (_dir, store) = keyed_store();
        let mut session = stale_session();
        session.access_token_issued_at_ms = now_ms();

        refresh_stale_access_token(
            &mut session,
            &http(&server.base_url()),
            &*store,
            CLIENT_ID,
            None,
        )
        .await
        .expect("a fresh token must be a no-op");

        rebind.assert_calls(0);
        assert_eq!(
            session.access_token, "old.jwt.token",
            "a fresh token must be left untouched"
        );
    }

    /// Dead zuid → `/access` rejects → error surfaces (re-login), and
    /// the rebind was actually attempted.
    /// Step 95c: `clear_persisted_tokens` blanks the access token after
    /// `SessionExpired`; on wasm (no refresh-token guard) a resume
    /// within the token's lifetime used to keep `""` as "fresh" and
    /// carry it into the next call's `debug_assert!`. An empty access
    /// token is stale however recent its stamp.
    #[tokio::test]
    async fn an_empty_access_token_is_stale_however_recent() {
        let server = MockServer::start();
        let rebind = mount_rebind(&server);
        let (_dir, store) = keyed_store();
        let mut session = stale_session();
        session.access_token = String::new();
        session.access_token_issued_at_ms = now_ms();

        refresh_stale_access_token(
            &mut session,
            &http(&server.base_url()),
            &*store,
            CLIENT_ID,
            None,
        )
        .await
        .expect("an empty token is rebound");

        rebind.assert_calls(1);
        assert_eq!(session.access_token, "new.jwt.token");
    }

    /// Step 95c: a persisted `devices.mls_public_key` of `""` decoded to an
    /// empty key and reached `resume_with_storage`'s `debug_assert!`.
    #[test]
    fn an_empty_persisted_mls_public_key_is_invalid_input() {
        let err = decode_persisted_mls_public_key(Some("")).expect_err("no key");

        assert!(
            matches!(err, KupferdrahtError::InvalidInput(_)),
            "got {err:?}"
        );
    }

    /// Step 95c: the persisted device row is runtime data — a row with
    /// an empty `client_id` (never written by a correct build, but a
    /// foreign or damaged DB can hold one) used to hit a `debug_assert!`.
    /// It is a typed storage error now, and no rebind is attempted.
    #[tokio::test]
    async fn an_empty_persisted_client_id_is_a_storage_error() {
        let server = MockServer::start();
        let rebind = mount_rebind(&server);
        let (_dir, store) = keyed_store();
        let mut session = stale_session();

        let err =
            refresh_stale_access_token(&mut session, &http(&server.base_url()), &*store, "", None)
                .await
                .expect_err("an empty client_id cannot be resumed");

        assert!(matches!(err, KupferdrahtError::Storage(_)), "got {err:?}");
        rebind.assert_calls(0);
    }

    #[tokio::test]
    async fn surfaces_error_when_rebind_fails() {
        let server = MockServer::start();
        let rebind = server.mock(|when, then| {
            when.method(POST).path("/v14/access");
            then.status(403)
                .header("content-type", "application/json")
                .body(r#"{"code":403,"label":"invalid-credentials","message":"dead zuid"}"#);
        });
        let (_dir, store) = keyed_store();
        let mut session = stale_session();

        let err = refresh_stale_access_token(
            &mut session,
            &http(&server.base_url()),
            &*store,
            CLIENT_ID,
            None,
        )
        .await
        .expect_err("a dead zuid must surface an error so the caller re-logs-in");

        // Step 39: a 403 invalid-credentials on the rebind is the terminal
        // session-expiry signal, typed distinctly so the caller drives re-login.
        assert!(
            matches!(err, KupferdrahtError::SessionExpired),
            "expected SessionExpired, got {err:?}"
        );
        rebind.assert_calls(1);
    }
}
