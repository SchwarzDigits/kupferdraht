// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

//! Step 142 — openmls's migrations run atomically in `mls.db`.
//!
//! `KupferdrahtProvider::open` runs `openmls_sqlite_storage`'s
//! migrations. With refinery's default runner a migration's SQL and its
//! history row commit separately: a process that dies between the two
//! leaves the migration applied but unrecorded, the next open applies
//! it again, and from V3 on that fails for good (`CREATE TABLE` of a
//! table that exists). The fork's grouped runner applies the whole run
//! in one transaction, so an interrupted run leaves the store as it was.
//!
//! The crash is a SQLite trigger on refinery's history table that
//! aborts one history-row insert, created before the first open.

use std::path::Path;

use openmls_sqlite_storage::Connection;

use super::provider::KupferdrahtProvider;
use crate::error::KupferdrahtError;

/// refinery's history table for openmls's migrations, under the name
/// `run_migrations` gives it.
const HISTORY: &str = "openmls_sqlite_storage_migrations";

/// The table openmls's V3 (`V3__virtual_clients.sql`) creates.
const V3_TABLE: &str = "vc_emulation_group_secrets";

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |row| row.get::<_, i64>(0),
    )
    .expect("sqlite_master readable")
        > 0
}

fn recorded_up_to(conn: &Connection, version: u32) -> i64 {
    conn.query_row(
        &format!("SELECT COUNT(*) FROM {HISTORY} WHERE version <= ?1"),
        [version],
        |row| row.get(0),
    )
    .expect("history table readable")
}

/// The state a crash between a migration's SQL and its history row
/// leaves: refinery's history table, created ahead of the run (refinery
/// only creates it if it is missing), with a trigger that aborts the
/// insert of `version`'s row after that migration's SQL ran.
fn interrupt_before_history_row(path: &Path, version: u32) {
    let conn = Connection::open(path).expect("open mls.db");
    conn.execute_batch(&format!(
        "CREATE TABLE {HISTORY} (
             version INT4 PRIMARY KEY,
             name VARCHAR(255),
             applied_on VARCHAR(255),
             checksum VARCHAR(255)
         );
         CREATE TRIGGER interrupt BEFORE INSERT ON {HISTORY}
         WHEN NEW.version = {version}
         BEGIN SELECT RAISE(ABORT, 'interrupted before the history row'); END;"
    ))
    .expect("history table and trigger");
}

#[test]
fn an_interrupted_migration_run_leaves_mls_db_openable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("mls.db");
    interrupt_before_history_row(&path, 3);

    let interrupted = KupferdrahtProvider::open(Connection::open(&path).expect("open mls.db"));
    assert!(
        matches!(interrupted, Err(KupferdrahtError::Storage(_))),
        "the run must fail at V3's history row, got {interrupted:?}"
    );

    let conn = Connection::open(&path).expect("reopen mls.db");
    assert!(
        !table_exists(&conn, V3_TABLE),
        "V3 must not stay applied without its history row"
    );
    assert_eq!(
        recorded_up_to(&conn, 3),
        0,
        "the interrupted run must record nothing"
    );
    conn.execute_batch("DROP TRIGGER interrupt;")
        .expect("drop the trigger");
    drop(conn);

    KupferdrahtProvider::open(Connection::open(&path).expect("reopen mls.db"))
        .expect("the next open migrates from the state before the interrupted run");
    let conn = Connection::open(&path).expect("reopen mls.db");
    assert!(table_exists(&conn, V3_TABLE), "V3 applied");
    assert_eq!(recorded_up_to(&conn, 3), 3, "V1 to V3 recorded");
}
