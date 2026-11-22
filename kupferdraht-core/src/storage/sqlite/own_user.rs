// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! The own user's profile row, CRUD. Whole-DB encryption protects the
//! `email` column at rest, so it is stored and read verbatim like
//! the rest (`id`, the username, `name`). `id` holds the
//! `<localpart>@<domain>` user id (Step 19).

use rusqlite::{OptionalExtension, params};

use crate::avatar::AvatarSource;
use crate::error::KupferdrahtError;
use crate::storage::records::OwnUserRecord;

use super::{SqliteStore, utf8_column};

pub(super) fn save_own_user(
    store: &SqliteStore,
    user: &OwnUserRecord,
) -> Result<(), KupferdrahtError> {
    debug_assert!(!user.id.is_empty(), "own user id must be non-empty");
    let conn = store.lock()?;
    conn.execute(
        "INSERT INTO self_user (
            id, handle, name, email, team_id, team_name,
            avatar_thumbnail_key, avatar_full_key, team_avatar_key
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(id) DO UPDATE SET
             handle    = excluded.handle,
             name      = excluded.name,
             email     = excluded.email,
             team_id   = excluded.team_id,
             team_name = excluded.team_name,
             avatar_thumbnail_key = excluded.avatar_thumbnail_key,
             avatar_full_key      = excluded.avatar_full_key,
             team_avatar_key      = excluded.team_avatar_key",
        params![
            user.id,
            user.username,
            user.name,
            user.email.as_deref().map(str::as_bytes),
            user.team_id,
            user.team_name,
            user.avatar_source.thumbnail,
            user.avatar_source.full,
            user.team_avatar_version,
        ],
    )
    .map_err(|e| KupferdrahtError::Storage(format!("save_own_user: {e}")))?;
    Ok(())
}

pub(super) fn load_own_user(
    store: &SqliteStore,
    id: &str,
) -> Result<Option<OwnUserRecord>, KupferdrahtError> {
    debug_assert!(!id.is_empty(), "id must be non-empty");
    let raw = {
        let conn = store.lock()?;
        conn.query_row(
            "SELECT handle, name, email, team_id, team_name,
                    avatar_thumbnail_key, avatar_full_key, team_avatar_key
             FROM self_user WHERE id = ?1",
            params![id],
            |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<Vec<u8>>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    AvatarSource {
                        thumbnail: row.get(5)?,
                        full: row.get(6)?,
                    },
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()
        .map_err(|e| KupferdrahtError::Storage(format!("load_own_user: {e}")))?
    };
    let Some((username, name, email, team_id, team_name, avatar_source, team_avatar_version)) = raw
    else {
        return Ok(None);
    };
    let email = email.map(|b| utf8_column(b, "email")).transpose()?;
    Ok(Some(OwnUserRecord {
        id: id.to_string(),
        username,
        name,
        email,
        team_id,
        team_name,
        avatar_source,
        team_avatar_version,
    }))
}
