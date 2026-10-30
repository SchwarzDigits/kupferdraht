// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `kupferdraht-cli` authentication + session-lifecycle command handlers.

use std::sync::Arc;

use kupferdraht::storage::{
    BackendMlsKey, DeviceRecord, SelfUserRecord, ServerConfigRecord, SessionRecord,
    StorageEngineInfo, Store,
};
use kupferdraht::{
    Client, ClientBuilder, Kupferdraht, KupferdrahtError, PendingLogin, ServerConfig, UserId,
    now_ms,
};

use super::{
    base64_encode, cli_keystore, kupferdraht_home, open_manager, prefix, read_password,
    resume_active, resume_client,
};

/// SSO (Step 17β) — validate `code` against the backend and print the
/// interactive initiate URL to open in a browser. The post-`IdP` redirect
/// carries the `zuid` cookie that `sso finalize` (Step 17b) consumes.
pub(crate) async fn sso_initiate(
    server: ServerConfig,
    code: Option<String>,
    email: Option<String>,
    success_redirect: String,
    error_redirect: String,
) -> Result<(), KupferdrahtError> {
    let app = Kupferdraht::open(kupferdraht_home()?, cli_keystore())?;
    let url = match (code, email) {
        (Some(code), _) => {
            app.initiate_sso(server, &code, &success_redirect, &error_redirect)
                .await?
        }
        (None, Some(email)) => {
            app.initiate_sso_by_email(server, &email, &success_redirect, &error_redirect)
                .await?
        }
        (None, None) => {
            return Err(KupferdrahtError::InvalidInput(
                "provide --code or --email".into(),
            ));
        }
    };
    println!("sso initiate ok");
    println!("  open this URL in a browser to authenticate:");
    println!("    {url}");
    println!();
    println!("After the IdP round-trip, capture the zuid cookie from the redirect");
    println!("and run `kupferdraht-cli sso finalize` (Step 17b).");
    Ok(())
}

/// SSO finalize (Step 17β) — mint the session from the `zuid` cookie, register
/// this device, and persist the account. Mirrors `login`'s phase-2 device
/// registration (including the 7-device-cap recovery).
pub(crate) async fn sso_finalize(
    server: ServerConfig,
    cookie: String,
    delete_device: Option<String>,
) -> Result<(), KupferdrahtError> {
    let app = Kupferdraht::open(kupferdraht_home()?, cli_keystore())?;
    let pending = app.finalize_sso(server, &cookie, None).await?;
    let client = match pending.register_device().await {
        Ok(client) => client,
        Err(KupferdrahtError::TooManyDevices) => {
            match recover_device_limit(&pending, delete_device).await? {
                Some(client) => client,
                None => return Ok(()),
            }
        }
        Err(e) => return Err(e),
    };
    let account = client.account();
    println!("sso finalize ok");
    println!(
        "  user_id:     {}",
        client.user_id().map_or("—", UserId::as_str)
    );
    println!("  device_id:   {}", client.device_id().unwrap_or("—"));
    println!("  handle:      {}", account.handle().unwrap_or("—"));
    println!("  name:        {}", account.display_name());
    println!("  api_version: {}", client.api_version());
    println!("  server:      {}", client.server_title());
    Ok(())
}

pub(crate) async fn login(
    server: ServerConfig,
    email: String,
    password: String,
    delete_device: Option<String>,
) -> Result<(), KupferdrahtError> {
    // The device manager logs in, registers the account in the device
    // registry (so `accounts` lists it), marks it active, and persists
    // the session — the CLI never constructs the store or keystore beyond
    // handing the manager the device keystore.
    let app = Kupferdraht::open(kupferdraht_home()?, cli_keystore())?;
    // Phase 1: authenticate. Auth errors (wrong password, missing 2FA)
    // surface here.
    let pending = app.login(server, email, password, None, None).await?;
    // Phase 2: register this device. Wire's per-user device cap surfaces
    // here and is recoverable on the same captured session — no second login.
    let client = match pending.register_device().await {
        Ok(client) => client,
        Err(KupferdrahtError::TooManyDevices) => {
            match recover_device_limit(&pending, delete_device).await? {
                Some(client) => client,
                None => return Ok(()),
            }
        }
        Err(e) => return Err(e),
    };
    let account = client.account();
    println!("login ok");
    println!(
        "  user_id:     {}",
        client.user_id().map_or("—", UserId::as_str)
    );
    println!("  device_id:   {}", client.device_id().unwrap_or("—"));
    println!("  handle:      {}", account.handle().unwrap_or("—"));
    println!("  name:        {}", account.display_name());
    println!("  api_version: {}", client.api_version());
    println!("  server:      {}", client.server_title());
    Ok(())
}

/// Recover from `TooManyDevices` on the same `PendingLogin` — no second
/// login, since the session was captured in phase 1. With `delete_device`,
/// removes that device and retries `register_device` (returns the new
/// `Client`); without it, prints the registered devices and returns `None`
/// so the caller can stop.
pub(crate) async fn recover_device_limit(
    pending: &PendingLogin,
    delete_device: Option<String>,
) -> Result<Option<Arc<Client>>, KupferdrahtError> {
    match delete_device {
        Some(id) => {
            pending.delete_device(&id).await?;
            println!("device-limit: deleted {id}, retrying device registration…");
            Ok(Some(pending.register_device().await?))
        }
        None => {
            println!("device limit reached — too many registered devices. Registered devices:");
            for d in pending.devices().await? {
                println!(
                    "  {}  label={}  model={}",
                    d.device_id,
                    d.label.as_deref().unwrap_or("—"),
                    d.model.as_deref().unwrap_or("—"),
                );
            }
            println!("Re-run `login … --delete-device <id>` to free a slot and retry.");
            Ok(None)
        }
    }
}

/// `accounts` — list the accounts registered on this device. Reads the
/// device `GlobalStore` only, so it needs no account unlock.
pub(crate) async fn accounts() -> Result<(), KupferdrahtError> {
    let app = Kupferdraht::open(kupferdraht_home()?, cli_keystore())?;
    let entries = app.accounts().await?;
    if entries.is_empty() {
        println!("no accounts on this device — run `kupferdraht-cli login` first");
        return Ok(());
    }
    let active = app.active_account().await?;
    for entry in &entries {
        let marker = if active.as_deref() == Some(entry.account_id.as_str()) {
            "* "
        } else {
            "  "
        };
        println!("{marker}{}", entry.account_id);
        if let Some(handle) = &entry.handle {
            println!("    handle:  @{handle}");
        }
        if let Some(name) = &entry.display_name {
            println!("    name:    {name}");
        }
        println!("    server:  {}", entry.server);
    }
    Ok(())
}

/// Persist the post-login artefacts a runtime `Client` carries into
/// `kupferdraht.db` so subsequent `kupferdraht-cli` invocations can `resume`
/// without re-entering the password. Shared by `login` (Step 1)
/// and `activate` (Step 16d) since both terminate in the same
/// fully-unlocked `Client` state.
pub(crate) async fn persist_session_state(
    client: &Arc<Client>,
    store: &Arc<dyn Store>,
    server_config: &ServerConfig,
    cookie_label: Option<String>,
) -> Result<(), KupferdrahtError> {
    let user_id = client
        .user_id()
        .map_or_else(String::new, |u| u.as_str().to_string());
    let device_id = client.device_id().unwrap_or_default().to_string();
    let account = client.account();
    let session = SessionRecord {
        user_id: user_id.clone(),
        device_id: device_id.clone(),
        access_token: client.access_token().unwrap_or_default(),
        refresh_token: client.refresh_token().unwrap_or_default(),
        cookie_label,
        api_version: client.api_version(),
        access_token_issued_at_ms: now_ms(),
    };
    store.save_session(&session).await?;
    let device = DeviceRecord {
        user_id: user_id.clone(),
        client_id: device_id.clone(),
        mls_public_key: Some(base64_encode(client.mls_signature_public_key())),
    };
    store.save_device(&device).await?;
    store
        .save_self_user(&SelfUserRecord {
            id: user_id,
            handle: account.handle().map(str::to_string),
            name: account.display_name().to_string(),
            email: account.email().map(str::to_string),
        })
        .await?;
    store
        .save_server_config(&ServerConfigRecord {
            api_base_url: server_config.api_base_url.clone(),
            websocket_url: server_config.websocket_url.clone(),
            title: server_config.title.clone(),
        })
        .await?;
    let backend_removal_key = BackendMlsKey {
        purpose: "removal".to_string(),
        signature_scheme: "ecdsa_secp256r1_sha256".to_string(),
        public_key: client.backend_removal_key_ecdsa_p256().to_vec(),
    };
    store.save_backend_mls_key(&backend_removal_key).await?;
    Ok(())
}

pub(crate) async fn register(
    server_config: ServerConfig,
    email: String,
    name: String,
    password: String,
) -> Result<(), KupferdrahtError> {
    let storage_root = kupferdraht_home()?;
    std::fs::create_dir_all(&storage_root).map_err(|e| {
        KupferdrahtError::Environment(format!("mkdir {}: {e}", storage_root.display()))
    })?;
    // `register` does not persist anything locally — the PendingClient
    // is in-memory only (CLAUDE.md §What NOT to do). The caller must
    // run `kupferdraht-cli activate --email --code --password-stdin` to
    // complete the loop, after which the regular session state is
    // written by `persist_session_state`.
    let pending = ClientBuilder::new()
        .server(server_config)
        .base_dir(storage_root)
        .register(name, email.clone(), password)
        .await?;
    println!("register ok — account pending activation");
    println!("  user_id: {}", pending.user_id());
    println!("  email:   {}", pending.email());
    println!();
    println!("Next: run");
    println!(
        "  kupferdraht-cli activate --api-url <url> --email {} --code <CODE> [--password-stdin]",
        pending.email()
    );
    println!("once the activation email has arrived.");
    Ok(())
}

pub(crate) async fn activate(
    server_config: ServerConfig,
    email: String,
    code: String,
    password: String,
) -> Result<(), KupferdrahtError> {
    let storage_root = kupferdraht_home()?;
    let client = ClientBuilder::new()
        .server(server_config.clone())
        .base_dir(storage_root)
        .activate(email, code, password)
        .await?;
    let store = client
        .store()
        .ok_or_else(|| KupferdrahtError::Storage("activate: default store missing".into()))?;
    // Step 93: the session row records the home's label it logged in with.
    let cookie_label = open_manager()?.cookie_label();
    persist_session_state(&client, &store, &server_config, Some(cookie_label)).await?;
    let account = client.account();
    println!("activate ok");
    println!(
        "  user_id:     {}",
        client.user_id().map_or("—", UserId::as_str)
    );
    println!("  device_id:   {}", client.device_id().unwrap_or("—"));
    println!("  handle:      {}", account.handle().unwrap_or("—"));
    println!("  name:        {}", account.display_name());
    println!("  api_version: {}", client.api_version());
    println!("  server:      {}", client.server_title());
    Ok(())
}

pub(crate) async fn change_password() -> Result<(), KupferdrahtError> {
    let old = rpassword::prompt_password("Current account password: ")
        .map_err(|e| KupferdrahtError::Generic(format!("read current password: {e}")))?;
    let new = rpassword::prompt_password("New password: ")
        .map_err(|e| KupferdrahtError::Generic(format!("read new password: {e}")))?;
    let confirm = rpassword::prompt_password("New password (again): ")
        .map_err(|e| KupferdrahtError::Generic(format!("read new-password confirmation: {e}")))?;
    if new != confirm {
        return Err(KupferdrahtError::InvalidInput(
            "change-password: the two new-password entries do not match".into(),
        ));
    }
    let client = resume_client().await?;
    client.change_password(&old, &new).await?;
    println!("change-password ok");
    println!("  every device's session is now invalidated — other devices must re-login.");
    println!("  this session keeps working until its access token expires (~15 min).");
    Ok(())
}

pub(crate) async fn logout(
    purge_device: bool,
    password_stdin: bool,
) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    let device_id = client.device_id().unwrap_or_default().to_string();
    let password = if purge_device {
        let pw = read_password(password_stdin)
            .map_err(|e| KupferdrahtError::Environment(format!("read password: {e}")))?;
        if pw.is_empty() {
            return Err(KupferdrahtError::InvalidInput(
                "logout: --purge-device requires a non-empty password".into(),
            ));
        }
        Some(pw)
    } else {
        None
    };
    client.logout(purge_device, password.as_deref()).await?;
    let home = kupferdraht_home()?;
    if purge_device && home.exists() {
        std::fs::remove_dir_all(&home).map_err(|e| {
            KupferdrahtError::Environment(format!("rm -rf {}: {e}", home.display()))
        })?;
    }
    println!("logout ok");
    println!("  device_id:    {device_id}");
    println!("  purge_device: {purge_device}");
    println!("{}", local_state_lines(purge_device, &home));
    Ok(())
}

/// The `logout` local-state lines: with `--purge-device` the home that was
/// actually wiped — `$KUPFERDRAHT_HOME` or the platform default, never a fixed
/// path — else the preservation note. Pure, so the format is unit-tested.
pub(crate) fn local_state_lines(purge_device: bool, home: &std::path::Path) -> String {
    if purge_device {
        format!(
            "  local state:  wiped ({})\n  next login:   will register a fresh device + MLS identity",
            home.display()
        )
    } else {
        "  local state:  preserved (next login reuses this device)".to_owned()
    }
}

pub(crate) async fn status() -> Result<(), KupferdrahtError> {
    // The store is whole-DB-encrypted and opened ready by the
    // provider — there is no separate unlock step. Go through the same
    // `resume_client()` path every other CLI subcommand uses rather
    // than opening the store inline.
    if !kupferdraht_db_exists()? {
        println!("no session persisted — run `kupferdraht-cli login` first");
        return Ok(());
    }
    let app = open_manager()?;
    let (client, store) = resume_active(&app).await?;
    let user_id = client.user_id().map_or("—", UserId::as_str);
    let device_id = client.device_id().unwrap_or("—");
    let access_token = client.access_token().unwrap_or_default();
    let device = store.load_device(user_id, device_id).await?;
    // Step 93: the label the persisted session was logged in with — `—`
    // for a session that predates its home's own label.
    let session_label = store
        .load_session(user_id)
        .await?
        .and_then(|session| session.cookie_label);
    let engine = client.storage_engine().await?;
    println!("session:");
    println!("  user_id:                   {user_id}");
    println!("  device_id:                 {device_id}");
    println!("  api_version:               {}", client.api_version());
    println!("  access_token_prefix:       {}", prefix(&access_token, 12));
    if let Some(dev) = device.as_ref().and_then(|d| d.mls_public_key.as_deref()) {
        println!("  mls_public_key_prefix:     {}", prefix(dev, 16));
    } else {
        println!("  mls_public_key_prefix:     —");
    }
    // Step 95d: the device's MLS thumbprint, grouped like the Wire apps.
    println!(
        "  mls_thumbprint:            {}",
        client.own_device_thumbprint().map_or_else(
            |_| "—".to_owned(),
            |tp| super::rooms::grouped_thumbprint(&tp)
        )
    );
    println!(
        "{}",
        home_lines(app.home_id(), &app.cookie_label(), session_label.as_deref())
    );
    println!("{}", storage_line(engine.as_ref()));
    Ok(())
}

/// The `status` home lines (Step 93): the home id, the cookie label in
/// effect for this home and the label the persisted session was logged in
/// with (`—` for a session that predates Step 93 — it still holds the old
/// shared label on Wire's side until its next login). Pure, so the format
/// is unit-tested.
pub(crate) fn home_lines(home_id: &str, cookie_label: &str, session_label: Option<&str>) -> String {
    format!(
        "  {:<27}{home_id}\n  {:<27}{cookie_label}\n  {:<27}{}",
        "home_id:",
        "cookie_label:",
        "session_label:",
        session_label.unwrap_or("—")
    )
}

/// The `status` storage line — engine release, cipher scheme and header
/// mode as the open store reports them (Step 90, ADR-0021), or a neutral
/// note when the backend reports no engine (a custom `StorageProvider`).
/// Pure, so the format is unit-tested.
pub(crate) fn storage_line(engine: Option<&StorageEngineInfo>) -> String {
    match engine {
        Some(info) => format!(
            "storage: {} {} · {} · plaintext header {}",
            info.engine, info.version, info.cipher, info.plaintext_header_size
        ),
        None => "storage: (engine not reported by this backend)".to_owned(),
    }
}

pub(crate) fn kupferdraht_db_exists() -> Result<bool, KupferdrahtError> {
    // Per-account layout: a session is persisted if any account directory
    // exists under `accounts/`.
    let accounts = kupferdraht_home()?.join("accounts");
    Ok(std::fs::read_dir(&accounts)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use kupferdraht::storage::StorageEngineInfo;

    use super::{home_lines, local_state_lines, storage_line};

    #[test]
    fn storage_line_names_engine_cipher_and_header_mode() {
        let info = StorageEngineInfo {
            engine: "SQLite3 Multiple Ciphers".into(),
            version: "2.5.1".into(),
            cipher: "chacha20".into(),
            plaintext_header_size: 32,
        };
        assert_eq!(
            storage_line(Some(&info)),
            "storage: SQLite3 Multiple Ciphers 2.5.1 · chacha20 · plaintext header 32"
        );
    }

    #[test]
    fn storage_line_is_neutral_when_the_backend_reports_no_engine() {
        assert_eq!(
            storage_line(None),
            "storage: (engine not reported by this backend)"
        );
    }

    /// Step 93: three aligned lines like the rest of `status`.
    #[test]
    fn home_lines_name_home_id_cookie_label_and_session_label() {
        assert_eq!(
            home_lines("0123abcd", "kupferdraht-0123abcd", Some("kupferdraht-old")),
            "  home_id:                   0123abcd\n  \
             cookie_label:              kupferdraht-0123abcd\n  \
             session_label:             kupferdraht-old"
        );
    }

    /// `logout --purge-device` names the home it wiped — the configured
    /// one, not a fixed `~/.kupferdraht/`.
    #[test]
    fn local_state_lines_name_the_wiped_home_or_the_preservation() {
        let home = std::path::Path::new("/tmp/kupferdraht-smoke-3b");
        assert_eq!(
            local_state_lines(true, home),
            "  local state:  wiped (/tmp/kupferdraht-smoke-3b)\n  \
             next login:   will register a fresh device + MLS identity"
        );
        assert_eq!(
            local_state_lines(false, home),
            "  local state:  preserved (next login reuses this device)"
        );
    }

    /// A session persisted before Step 93 has no recorded label.
    #[test]
    fn home_lines_show_a_dash_for_a_session_without_a_label() {
        assert!(
            home_lines("0123abcd", "kupferdraht-0123abcd", None)
                .ends_with("session_label:             —")
        );
    }
}
