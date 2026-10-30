// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `kupferdraht-cli` server-config command handlers (profile-set / features / properties).

use kupferdraht::KupferdrahtError;

use super::resume_client;

pub(crate) async fn profile_set(
    display_name: Option<String>,
    handle: Option<String>,
) -> Result<(), KupferdrahtError> {
    if display_name.is_none() && handle.is_none() {
        return Err(KupferdrahtError::InvalidInput(
            "profile-set: at least one of --display-name or --handle is required".into(),
        ));
    }
    let client = resume_client().await?;
    if let Some(name) = display_name.as_deref() {
        client.set_display_name(name).await?;
    }
    if let Some(handle) = handle.as_deref() {
        client.set_handle(handle).await?;
    }
    println!("profile-set ok");
    if let Some(name) = display_name.as_deref() {
        println!("  display_name: {name}");
    }
    if let Some(handle) = handle.as_deref() {
        println!("  handle:       {handle}");
    }
    Ok(())
}

/// `kupferdraht-cli email-update --email <addr>` (Step 98a).
///
/// Wire answers before the address changes: it mails a verification link and
/// only applies the change once that link is followed. The wording says so, so
/// nobody reads "ok" as "done".
pub(crate) async fn email_update(email: String) -> Result<(), KupferdrahtError> {
    if email.trim().is_empty() {
        return Err(KupferdrahtError::InvalidInput(
            "email-update: --email must not be empty".into(),
        ));
    }
    let client = resume_client().await?;
    client.update_email(email.trim()).await?;
    println!("email-update ok");
    println!("  verification sent to: {}", email.trim());
    println!("  the address changes once the link in that mail is followed");
    Ok(())
}

pub(crate) async fn features(name: Option<String>) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    if let Some(n) = name {
        match client.feature_config(&n).await? {
            Some(entry) => print_feature_config_detail(&entry),
            None => println!("(no feature config named \"{n}\")"),
        }
        return Ok(());
    }
    let entries = client.feature_configs().await?;
    if entries.is_empty() {
        println!("no feature configs — run `kupferdraht-cli login` first");
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

pub(crate) fn print_feature_config_detail(entry: &kupferdraht::storage::FeatureConfigEntry) {
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

pub(crate) async fn properties(key: Option<String>) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    if let Some(k) = key {
        match client.user_property(&k).await? {
            Some(entry) => print_user_property_detail(&entry),
            None => println!("(no user property named \"{k}\")"),
        }
        return Ok(());
    }
    let entries = client.user_properties().await?;
    if entries.is_empty() {
        println!("no user properties — none set on the backend, or login has not run");
        return Ok(());
    }
    let mut sorted = entries;
    sorted.sort_by(|a, b| a.key.cmp(&b.key));
    for e in &sorted {
        println!("{:<24} {}", e.key, brief_property_value(&e.value));
    }
    Ok(())
}

pub(crate) fn print_user_property_detail(entry: &kupferdraht::storage::UserPropertyEntry) {
    println!("key:                {}", entry.key);
    println!(
        "last_synced_at:     {}",
        super::fmt_ms(entry.last_synced_at_ms)
    );
    println!("value:              {}", entry.value);
}

/// Truncate large opaque property values (e.g. Wire-Web's `webapp`
/// blob can be multiple KB) for the brief listing — full value is
/// reachable via `properties --key <k>`.
pub(crate) fn brief_property_value(value: &str) -> String {
    const MAX_INLINE: usize = 80;
    if value.len() <= MAX_INLINE {
        return value.replace('\n', " ");
    }
    format!("<{} bytes — run with --key for full output>", value.len())
}
