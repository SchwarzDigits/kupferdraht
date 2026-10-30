// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `session` table CRUD: `save_session` and `load_session`. The
//! `client/resume.rs` resume path loads the session by the qualified
//! account id (`load_session(account_id)`) — the account is named
//! explicitly, so no table scan is needed. Whole-DB encryption protects
//! the token columns at rest, so they are stored and read verbatim.

use rusqlite::{OptionalExtension, params};

use crate::error::KupferdrahtError;
use crate::storage::records::SessionRecord;

use super::{SqliteStore, utf8_column};

const SESSION_UPSERT_SQL: &str = "INSERT INTO session (
    user_id, device_id, access_token, refresh_token,
    cookie_label, api_version, access_token_issued_at_ms
 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
 ON CONFLICT(user_id) DO UPDATE SET
    device_id                 = excluded.device_id,
    access_token              = excluded.access_token,
    refresh_token             = excluded.refresh_token,
    cookie_label              = excluded.cookie_label,
    api_version               = excluded.api_version,
    access_token_issued_at_ms = excluded.access_token_issued_at_ms";

pub(super) fn save_session(
    store: &SqliteStore,
    session: &SessionRecord,
) -> Result<(), KupferdrahtError> {
    debug_assert!(!session.user_id.is_empty(), "user_id must be non-empty");
    debug_assert!(!session.device_id.is_empty(), "device_id must be non-empty");
    let conn = store.lock()?;
    conn.execute(
        SESSION_UPSERT_SQL,
        params![
            session.user_id,
            session.device_id,
            session.access_token.as_bytes(),
            session.refresh_token.as_bytes(),
            session.cookie_label,
            session.api_version,
            session.access_token_issued_at_ms,
        ],
    )
    .map_err(|e| KupferdrahtError::Storage(format!("save_session: {e}")))?;
    Ok(())
}

pub(super) fn load_session(
    store: &SqliteStore,
    user_id: &str,
) -> Result<Option<SessionRecord>, KupferdrahtError> {
    debug_assert!(!user_id.is_empty(), "user_id must be non-empty");
    let raw = {
        let conn = store.lock()?;
        conn.query_row(
            "SELECT device_id, access_token, refresh_token,
                    cookie_label, api_version, access_token_issued_at_ms
             FROM session WHERE user_id = ?1",
            params![user_id],
            read_raw_session_row_by_uid(user_id),
        )
        .optional()
        .map_err(|e| KupferdrahtError::Storage(format!("load_session: {e}")))?
    };
    raw.map(assemble_session).transpose()
}

/// Raw column values read from a session row. Token columns are kept as
/// `Vec<u8>` so UTF-8 decoding can surface errors outside the `rusqlite`
/// row closure.
struct RawSessionRow {
    user_id: String,
    device_id: String,
    access_token: Vec<u8>,
    refresh_token: Vec<u8>,
    cookie_label: Option<String>,
    api_version: u32,
    access_token_issued_at_ms: i64,
}

fn read_raw_session_row_by_uid(
    user_id: &str,
) -> impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<RawSessionRow> + '_ {
    move |row| {
        Ok(RawSessionRow {
            user_id: user_id.to_string(),
            device_id: row.get(0)?,
            access_token: row.get(1)?,
            refresh_token: row.get(2)?,
            cookie_label: row.get(3)?,
            api_version: row.get(4)?,
            access_token_issued_at_ms: row.get(5)?,
        })
    }
}

fn assemble_session(raw: RawSessionRow) -> Result<SessionRecord, KupferdrahtError> {
    Ok(SessionRecord {
        user_id: raw.user_id,
        device_id: raw.device_id,
        access_token: utf8_column(raw.access_token, "access_token")?,
        refresh_token: utf8_column(raw.refresh_token, "refresh_token")?,
        cookie_label: raw.cookie_label,
        api_version: raw.api_version,
        access_token_issued_at_ms: raw.access_token_issued_at_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::super::{INVALID_UTF8, fresh_store};
    use crate::error::KupferdrahtError;
    use crate::storage::Store as _;
    use crate::storage::records::SessionRecord;
    use rusqlite::params;

    fn sample_session(user_id: &str) -> SessionRecord {
        SessionRecord {
            user_id: user_id.to_string(),
            device_id: "placeholder-device-id".to_string(),
            access_token: "placeholder.access.jwt".to_string(),
            refresh_token: "placeholder-refresh-cookie".to_string(),
            cookie_label: Some("kupferdraht-cli".to_string()),
            api_version: 14,
            access_token_issued_at_ms: 1_700_000_000_000,
        }
    }

    #[tokio::test]
    async fn load_session_surfaces_invalid_utf8_as_encoding_error() {
        let (_dir, store) = fresh_store();
        let session = sample_session("corrupt-access-user");
        store.save_session(&session).await.expect("save");

        {
            let conn = store.lock().expect("lock");
            conn.execute(
                "UPDATE session SET access_token = ?1 WHERE user_id = ?2",
                params![INVALID_UTF8, session.user_id],
            )
            .expect("corrupt access_token");
        }

        let err = store
            .load_session(&session.user_id)
            .await
            .expect_err("corrupted UTF-8 must surface as an error");
        assert!(
            matches!(err, KupferdrahtError::Encoding(_)),
            "corruption must surface as an Encoding error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn load_session_surfaces_invalid_refresh_token_utf8_as_encoding_error() {
        let (_dir, store) = fresh_store();
        let session = sample_session("corrupt-refresh-user");
        store.save_session(&session).await.expect("save");

        {
            let conn = store.lock().expect("lock");
            conn.execute(
                "UPDATE session SET refresh_token = ?1 WHERE user_id = ?2",
                params![INVALID_UTF8, session.user_id],
            )
            .expect("corrupt refresh_token");
        }

        let err = store
            .load_session(&session.user_id)
            .await
            .expect_err("corrupted UTF-8 must surface as an error");
        assert!(
            matches!(err, KupferdrahtError::Encoding(_)),
            "corruption must surface as an Encoding error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn load_session_surfaces_invalid_utf8_on_raw_inserted_row() {
        let (_dir, store) = fresh_store();

        {
            let conn = store.lock().expect("lock");
            conn.execute(
                "INSERT INTO session (
                    user_id, access_token, refresh_token, cookie_label,
                    api_version, access_token_issued_at_ms
                 ) VALUES (?1, ?2, ?3, NULL, 14, 1)",
                params!["raw-insert-user", INVALID_UTF8, INVALID_UTF8],
            )
            .expect("raw insert");
        }

        let err = store
            .load_session("raw-insert-user")
            .await
            .expect_err("corrupted UTF-8 must surface as an error");
        assert!(
            matches!(err, KupferdrahtError::Encoding(_)),
            "corruption must surface as an Encoding error, got {err:?}"
        );
    }
}
