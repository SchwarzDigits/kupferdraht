// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `kupferdraht-cli` profile and server-settings command handlers
//! (profile-set / email-change / settings / account-data).

use kupferdraht::KupferdrahtError;

use super::resume_client;

pub(crate) async fn profile_set(
    display_name: Option<String>,
    username: Option<String>,
) -> Result<(), KupferdrahtError> {
    if display_name.is_none() && username.is_none() {
        return Err(KupferdrahtError::InvalidInput(
            "profile-set: at least one of --display-name or --username is required".into(),
        ));
    }
    let client = resume_client().await?;
    if let Some(name) = display_name.as_deref() {
        client.set_display_name(name).await?;
    }
    if let Some(username) = username.as_deref() {
        client.set_username(username).await?;
    }
    println!("profile-set ok");
    if let Some(name) = display_name.as_deref() {
        println!("  display_name: {name}");
    }
    if let Some(username) = username.as_deref() {
        println!("  username:     {username}");
    }
    Ok(())
}

/// `kupferdraht-cli email-change --email <addr>` (Step 98a, renamed in 162).
///
/// The backend answers before the address changes: it mails a confirmation
/// link and only applies the change once that link is followed. The wording
/// says so, so nobody reads "ok" as "done".
pub(crate) async fn email_change(email: String) -> Result<(), KupferdrahtError> {
    if email.trim().is_empty() {
        return Err(KupferdrahtError::InvalidInput(
            "email-change: --email must not be empty".into(),
        ));
    }
    let client = resume_client().await?;
    client.request_email_change(email.trim()).await?;
    println!("email-change ok");
    println!("  verification sent to: {}", email.trim());
    println!("  the address changes once the link in that mail is followed");
    Ok(())
}

pub(crate) async fn settings(name: Option<String>) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    if let Some(n) = name {
        match client.server_setting(&n).await? {
            Some(entry) => print_server_setting_detail(&entry),
            None => println!("(no server setting named \"{n}\")"),
        }
        return Ok(());
    }
    let entries = client.server_settings().await?;
    if entries.is_empty() {
        println!("no server settings — run `kupferdraht-cli login` first");
        return Ok(());
    }
    let mut sorted = entries;
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for e in &sorted {
        let status = e.status.as_deref().unwrap_or("—");
        println!("{:<24} {:<10} {}", e.name, status, e.config_json);
    }
    Ok(())
}

pub(crate) fn print_server_setting_detail(entry: &kupferdraht::storage::ServerSetting) {
    println!("name:               {}", entry.name);
    println!(
        "status:             {}",
        entry.status.as_deref().unwrap_or("—")
    );
    println!(
        "last_synced_at:     {}",
        super::fmt_ms(entry.last_synced_at_ms)
    );
    println!("config_json:        {}", entry.config_json);
}

pub(crate) async fn account_data(key: Option<String>) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    if let Some(k) = key {
        match client.account_data_entry(&k).await? {
            Some(entry) => print_account_data_detail(&entry),
            None => println!("(no account-data entry named \"{k}\")"),
        }
        return Ok(());
    }
    let entries = client.account_data().await?;
    if entries.is_empty() {
        println!("no account data — none set on the backend, or login has not run");
        return Ok(());
    }
    let mut sorted = entries;
    sorted.sort_by(|a, b| a.key.cmp(&b.key));
    for e in &sorted {
        println!("{:<24} {}", e.key, brief_value(&e.value));
    }
    Ok(())
}

pub(crate) fn print_account_data_detail(entry: &kupferdraht::storage::AccountDataEntry) {
    println!("key:                {}", entry.key);
    println!(
        "last_synced_at:     {}",
        super::fmt_ms(entry.last_synced_at_ms)
    );
    println!("value:              {}", entry.value);
}

/// Truncate large opaque account-data values (e.g. Wire-Web's `webapp`
/// blob can be multiple KB) for the brief listing — full value is
/// reachable via `account-data --key <k>`.
pub(crate) fn brief_value(value: &str) -> String {
    const MAX_INLINE: usize = 80;
    if value.len() <= MAX_INLINE {
        return value.replace('\n', " ");
    }
    format!("<{} bytes — run with --key for full output>", value.len())
}
