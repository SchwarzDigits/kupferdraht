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
            avatar_thumbnail_key, avatar_full_key, team_avatar_key,
            supported_protocols
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
             handle    = excluded.handle,
             name      = excluded.name,
             email     = excluded.email,
             team_id   = excluded.team_id,
             team_name = excluded.team_name,
             avatar_thumbnail_key = excluded.avatar_thumbnail_key,
             avatar_full_key      = excluded.avatar_full_key,
             team_avatar_key      = excluded.team_avatar_key,
             supported_protocols  = excluded.supported_protocols",
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
            protocols_column(&user.supported_protocols),
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
                    avatar_thumbnail_key, avatar_full_key, team_avatar_key,
                    supported_protocols
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
                    row.get::<_, Option<String>>(8)?,
                ))
            },
        )
        .optional()
        .map_err(|e| KupferdrahtError::Storage(format!("load_own_user: {e}")))?
    };
    let Some((
        username,
        name,
        email,
        team_id,
        team_name,
        avatar_source,
        team_avatar_version,
        protocols,
    )) = raw
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
        supported_protocols: protocols_from_column(protocols.as_deref())?,
    }))
}

/// The protocol list as the `supported_protocols` column holds it: a JSON
/// array of strings, `NULL` for an empty list (a row of a login before
/// Step 167 reads NULL too).
fn protocols_column(protocols: &[String]) -> Option<String> {
    if protocols.is_empty() {
        return None;
    }
    serde_json::to_string(protocols).ok()
}

/// The inverse of [`protocols_column`]; a column this code did not write
/// (not a JSON array of strings) is a storage error, not a panic.
fn protocols_from_column(column: Option<&str>) -> Result<Vec<String>, KupferdrahtError> {
    match column {
        None => Ok(Vec::new()),
        Some(json) => serde_json::from_str(json)
            .map_err(|e| KupferdrahtError::Storage(format!("self_user.supported_protocols: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::super::fresh_store;
    use super::*;
    use crate::storage::Store as _;

    fn row(protocols: &[&str]) -> OwnUserRecord {
        OwnUserRecord {
            id: "u@d".into(),
            username: None,
            name: "U".into(),
            email: None,
            team_id: None,
            team_name: None,
            avatar_source: AvatarSource::default(),
            team_avatar_version: None,
            supported_protocols: protocols.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    /// The list round-trips; an empty one is NULL and reads back empty.
    #[tokio::test]
    async fn the_protocol_list_round_trips_and_an_empty_one_is_null() {
        let (_dir, store) = fresh_store();
        store.save_own_user(&row(&["mls"])).await.expect("save");
        let loaded = store
            .load_own_user("u@d")
            .await
            .expect("load")
            .expect("row");
        assert_eq!(loaded.supported_protocols, ["mls"]);

        store.save_own_user(&row(&[])).await.expect("save empty");
        let loaded = store
            .load_own_user("u@d")
            .await
            .expect("load")
            .expect("row");
        assert!(loaded.supported_protocols.is_empty());
        let raw: Option<String> = store
            .lock()
            .expect("lock")
            .query_row(
                "SELECT supported_protocols FROM self_user WHERE id = 'u@d'",
                [],
                |r| r.get(0),
            )
            .expect("column");
        assert_eq!(raw, None, "an empty list is stored as NULL");
    }

    /// A row a login before Step 167 wrote (NULL) reads an empty list.
    #[tokio::test]
    async fn a_row_without_the_column_value_reads_empty() {
        let (_dir, store) = fresh_store();
        store.save_own_user(&row(&["mls"])).await.expect("save");
        store
            .lock()
            .expect("lock")
            .execute(
                "UPDATE self_user SET supported_protocols = NULL WHERE id = 'u@d'",
                [],
            )
            .expect("null it");
        let loaded = store
            .load_own_user("u@d")
            .await
            .expect("load")
            .expect("row");
        assert!(loaded.supported_protocols.is_empty());
    }
}
