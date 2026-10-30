// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `self_user` table CRUD. Whole-DB encryption protects the
//! `email` column at rest, so it is stored and read verbatim like
//! the rest (`id`, `handle`, `name`). `id` holds the qualified
//! `<localpart>@<domain>` user id (Step 19).

use rusqlite::{OptionalExtension, params};

use crate::error::KupferdrahtError;
use crate::storage::records::SelfUserRecord;

use super::{SqliteStore, utf8_column};

pub(super) fn save_self_user(
    store: &SqliteStore,
    user: &SelfUserRecord,
) -> Result<(), KupferdrahtError> {
    debug_assert!(!user.id.is_empty(), "self_user.id must be non-empty");
    let conn = store.lock()?;
    conn.execute(
        "INSERT INTO self_user (
            id, handle, name, email
         ) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(id) DO UPDATE SET
             handle = excluded.handle,
             name   = excluded.name,
             email  = excluded.email",
        params![
            user.id,
            user.handle,
            user.name,
            user.email.as_deref().map(str::as_bytes),
        ],
    )
    .map_err(|e| KupferdrahtError::Storage(format!("save_self_user: {e}")))?;
    Ok(())
}

pub(super) fn load_self_user(
    store: &SqliteStore,
    id: &str,
) -> Result<Option<SelfUserRecord>, KupferdrahtError> {
    debug_assert!(!id.is_empty(), "id must be non-empty");
    let raw = {
        let conn = store.lock()?;
        conn.query_row(
            "SELECT handle, name, email
             FROM self_user WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|e| KupferdrahtError::Storage(format!("load_self_user: {e}")))?
    };
    let Some((handle, name, email)) = raw else {
        return Ok(None);
    };
    let email = email.map(|b| utf8_column(b, "email")).transpose()?;
    Ok(Some(SelfUserRecord {
        id: id.to_string(),
        handle,
        name,
        email,
    }))
}
