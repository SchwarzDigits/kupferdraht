// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! `backend_mls_keys` table CRUD. The `public_key` column holds raw MLS
//! public-key bytes; whole-DB encryption protects it at rest, so the bytes
//! are stored and read verbatim. `purpose` and `signature_scheme` form the
//! composite primary key.

use rusqlite::{OptionalExtension, params};

use crate::error::KupferdrahtError;
use crate::storage::records::BackendMlsKey;

use super::SqliteStore;

pub(super) fn save_backend_mls_key(
    store: &SqliteStore,
    key: &BackendMlsKey,
) -> Result<(), KupferdrahtError> {
    debug_assert!(!key.purpose.is_empty(), "purpose must be non-empty");
    debug_assert!(
        !key.signature_scheme.is_empty(),
        "signature_scheme must be non-empty"
    );
    debug_assert!(!key.public_key.is_empty(), "public_key must be non-empty");
    let conn = store.lock()?;
    conn.execute(
        "INSERT INTO backend_mls_keys (
            purpose, signature_scheme, public_key
         ) VALUES (?1, ?2, ?3)
         ON CONFLICT(purpose, signature_scheme) DO UPDATE SET
             public_key = excluded.public_key",
        params![key.purpose, key.signature_scheme, key.public_key],
    )
    .map_err(|e| KupferdrahtError::Storage(format!("save_backend_mls_key: {e}")))?;
    Ok(())
}

pub(super) fn load_backend_mls_key(
    store: &SqliteStore,
    purpose: &str,
    signature_scheme: &str,
) -> Result<Option<BackendMlsKey>, KupferdrahtError> {
    debug_assert!(!purpose.is_empty(), "purpose must be non-empty");
    debug_assert!(
        !signature_scheme.is_empty(),
        "signature_scheme must be non-empty"
    );
    let public_key: Option<Vec<u8>> = {
        let conn = store.lock()?;
        conn.query_row(
            "SELECT public_key
             FROM backend_mls_keys
             WHERE purpose = ?1 AND signature_scheme = ?2",
            params![purpose, signature_scheme],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(|e| KupferdrahtError::Storage(format!("load_backend_mls_key: {e}")))?
    };
    let Some(public_key) = public_key else {
        return Ok(None);
    };
    Ok(Some(BackendMlsKey {
        purpose: purpose.to_string(),
        signature_scheme: signature_scheme.to_string(),
        public_key,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::fresh_store;
    use crate::storage::Store as _;
    use crate::storage::records::BackendMlsKey;

    #[tokio::test]
    async fn save_and_load_backend_mls_key_round_trip() {
        let (_dir, store) = fresh_store();
        let key = BackendMlsKey {
            purpose: "removal".to_string(),
            signature_scheme: "ecdsa_secp256r1_sha256".to_string(),
            public_key: vec![0x04, 0xDE, 0xAD, 0xBE, 0xEF],
        };

        store.save_backend_mls_key(&key).await.expect("save");
        let loaded = store
            .load_backend_mls_key(&key.purpose, &key.signature_scheme)
            .await
            .expect("load");
        assert_eq!(loaded, Some(key));
    }

    #[tokio::test]
    async fn load_backend_mls_key_returns_none_for_unknown_pair() {
        let (_dir, store) = fresh_store();
        let loaded = store
            .load_backend_mls_key("removal", "ecdsa_secp256r1_sha256")
            .await
            .expect("load");
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn save_backend_mls_key_is_upsert_on_same_pair() {
        let (_dir, store) = fresh_store();
        store
            .save_backend_mls_key(&BackendMlsKey {
                purpose: "removal".to_string(),
                signature_scheme: "ecdsa_secp256r1_sha256".to_string(),
                public_key: vec![0x01],
            })
            .await
            .expect("save #1");
        store
            .save_backend_mls_key(&BackendMlsKey {
                purpose: "removal".to_string(),
                signature_scheme: "ecdsa_secp256r1_sha256".to_string(),
                public_key: vec![0x02, 0x03],
            })
            .await
            .expect("save #2 (upsert)");

        let loaded = store
            .load_backend_mls_key("removal", "ecdsa_secp256r1_sha256")
            .await
            .expect("load");
        assert_eq!(loaded.expect("row").public_key, vec![0x02, 0x03]);
    }
}
