// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `kupferdraht-cli` device command handlers (devices / delete / rename /
//! key-packages).

use kupferdraht::{DeviceKind, KupferdrahtError};

use super::{explain_missing_password, fmt_ms, read_device_password, resume_client};

pub(crate) async fn devices() -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    let devices = client.devices().await?;
    println!("devices ok");
    println!("  registered: {}/7", devices.len());
    for d in &devices {
        let marker = if d.is_own { "*" } else { " " };
        let name = d.display_name.as_deref().unwrap_or("—");
        let last = d.last_seen_ms.map_or_else(|| "—".to_owned(), fmt_ms);
        println!(
            "  {marker} {} [{}] — {name} (last_seen: {last})",
            d.device_id,
            kind_name(d.kind)
        );
    }
    Ok(())
}

/// How `devices` prints a device's kind.
fn kind_name(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Phone => "phone",
        DeviceKind::Tablet => "tablet",
        DeviceKind::Desktop => "desktop",
        DeviceKind::LegalHold => "legal-hold",
        DeviceKind::Other => "other",
    }
}

pub(crate) async fn device_delete(
    device_id: String,
    password_stdin: bool,
) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    let password = read_device_password(password_stdin)
        .map_err(|e| KupferdrahtError::Environment(format!("read password: {e}")))?;
    client
        .delete_device(&device_id, password.as_deref())
        .await
        .map_err(explain_missing_password)?;
    println!("device-delete ok");
    println!("  device_id: {device_id}");
    Ok(())
}

pub(crate) async fn device_rename(
    device_id: String,
    display_name: String,
) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    client.rename_device(&device_id, &display_name).await?;
    println!("device-rename ok");
    println!("  device_id:    {device_id}");
    println!("  display_name: {display_name}");
    Ok(())
}

/// `kupferdraht-cli key-packages [--replenish]` — Step 63 pool diagnosis
/// and manual top-up (the sync loop also tops up once per day).
pub(crate) async fn key_packages(replenish: bool) -> Result<(), KupferdrahtError> {
    let client = resume_client().await?;
    let count = client.key_package_count().await?;
    println!("key-packages");
    println!("  server pool:  {count} (target 100, auto top-up below 50)");
    if replenish {
        let uploaded = client.replenish_key_packages().await?;
        if uploaded == 0 {
            println!("  replenish:    pool healthy — nothing uploaded");
        } else {
            println!("  replenish:    uploaded {uploaded}");
        }
    } else if count < 50 {
        println!("  status:       LOW — auto top-up on next sync, or run --replenish");
    }
    Ok(())
}
