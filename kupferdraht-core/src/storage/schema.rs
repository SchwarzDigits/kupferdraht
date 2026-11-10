// SPDX-FileCopyrightText: 2026 Schwarz Digits KG
// SPDX-License-Identifier: GPL-3.0-only

use rusqlite::Connection;

use crate::error::KupferdrahtError;

mod embedded {
    use refinery::embed_migrations;
    embed_migrations!("src/storage/migrations");
}

/// Run all pending migrations against the supplied connection.
///
/// Atomic (Step 87): every pending migration and its
/// `refinery_schema_history` row run in ONE transaction. refinery's
/// default commits each migration's SQL and its history row separately;
/// a crash between the two leaves the migration applied but unrecorded,
/// and the next open re-applies it — fatal for 25 of our 27 migrations
/// (`CREATE TABLE` without `IF NOT EXISTS`, unconditional `ALTER TABLE`).
/// `set_grouped` takes `self` by value: keep it in the builder chain, a
/// call on a binding whose result is discarded keeps the default.
///
/// Idempotent: calling this a second time on the same database performs
/// no work and returns `Ok(())`.
///
/// Step 157: a schema this build does not know is refused before anything
/// is applied — refinery's defaults (`abort_missing`, `abort_divergent`)
/// check every applied migration against the code first.
///
/// # Errors
/// `KupferdrahtError::IncompatibleStore` for a database newer than the code
/// or divergent from it; `KupferdrahtError::Storage` if refinery fails to
/// apply a migration — no migration is then applied or recorded (the whole
/// run rolls back).
pub(crate) fn run_migrations(conn: &mut Connection) -> Result<(), KupferdrahtError> {
    embedded::migrations::runner()
        .set_grouped(true)
        .run(conn)
        .map(|_report| ())
        .map_err(|e| {
            migration_refusal("kupferdraht.db", &e)
                .unwrap_or_else(|| KupferdrahtError::Storage(format!("migration failed: {e}")))
        })
}

/// Step 157: refinery's refusal of data this build does not fit, by name —
/// an applied migration the code lacks is a database `"newer"` than the
/// code, one whose content differs is `"divergent"`. `None` for every other
/// failure.
pub(crate) fn migration_refusal(database: &str, e: &refinery::Error) -> Option<KupferdrahtError> {
    let reason = match e.kind() {
        refinery::error::Kind::MissingVersion(_) => "newer",
        refinery::error::Kind::DivergentVersion(..) => "divergent",
        _ => return None,
    };
    Some(KupferdrahtError::IncompatibleStore {
        database: database.to_owned(),
        reason: reason.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    //! Step 87 — atomic schema migrations, pinned by fault injection.
    //!
    //! refinery's rusqlite driver commits one transaction per `execute`
    //! call. In the non-grouped default a migration's SQL and its
    //! `refinery_schema_history` row are TWO executes — two commits. A
    //! crash between them leaves the migration applied but unrecorded;
    //! the next open re-applies it, and 25 of our 27 migrations are
    //! re-apply-fatal (`CREATE TABLE` without `IF NOT EXISTS`,
    //! unconditional `ALTER TABLE`): the database is bricked for good.
    //!
    //! The proxy for "crash between the two commits" is a migration that
    //! FAILS after an earlier one succeeded. Non-grouped, the earlier
    //! migration and its history row survive the failed run. Grouped,
    //! every pending migration plus its history rows share one
    //! transaction, so a failure anywhere rolls the whole run back —
    //! the same transaction boundary that closes the crash window.

    use refinery::{Migration, Runner};
    use rusqlite::Connection;

    use super::{embedded, run_migrations};
    use crate::error::KupferdrahtError;

    fn table_exists(conn: &Connection, name: &str) -> bool {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
                [name],
                |row| row.get(0),
            )
            .expect("sqlite_master readable");
        count > 0
    }

    fn history_row_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM refinery_schema_history", [], |row| {
            row.get(0)
        })
        .expect("refinery_schema_history readable")
    }

    /// Fault: V2's table already exists, so V2 fails after V1 applied.
    fn connection_with_v2_conflict() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db");
        conn.execute_batch("CREATE TABLE devices (fault INTEGER);")
            .expect("inject the V2 conflict");
        conn
    }

    /// The red test: after a run that fails on V2, nothing of V1 may
    /// survive — no history row, no table. refinery creates the history
    /// table in its own preceding execute, so the right assertion is
    /// "exists but empty", not "absent".
    #[test]
    fn interrupted_run_leaves_no_partial_state() {
        let mut conn = connection_with_v2_conflict();

        let err = run_migrations(&mut conn).expect_err("V2 must fail on the injected conflict");
        assert!(matches!(err, KupferdrahtError::Storage(_)), "{err}");

        assert!(
            table_exists(&conn, "refinery_schema_history"),
            "refinery asserts its history table before the batch"
        );
        assert_eq!(
            history_row_count(&conn),
            0,
            "a failed run must record no migration — a recorded V1 means V1 was committed separately"
        );
        assert!(
            !table_exists(&conn, "session"),
            "V1's table must be rolled back together with the failed run"
        );
    }

    /// Preservation: once the fault is gone, the next run applies the
    /// full chain — a failed run never leaves the database un-openable.
    #[test]
    fn failed_run_can_be_retried_once_the_fault_is_gone() {
        let mut conn = connection_with_v2_conflict();
        run_migrations(&mut conn).expect_err("V2 must fail on the injected conflict");

        conn.execute_batch("DROP TABLE devices;")
            .expect("remove the injected fault");
        run_migrations(&mut conn).expect("retry after the fault is gone");

        let shipped = embedded::migrations::runner().get_migrations().len();
        let recorded = usize::try_from(history_row_count(&conn)).expect("non-negative count");
        assert_eq!(recorded, shipped, "every shipped migration is recorded");
        assert!(table_exists(&conn, "session"), "V1 applied");
        assert!(table_exists(&conn, "devices"), "V2 applied");
    }

    /// The refinery semantics `run_migrations` relies on, pinned with a
    /// test-local runner: grouped = one transaction for the batch, a
    /// failure anywhere rolls back every migration and history row.
    /// `set_grouped` takes `self` by value — it must stay in the builder
    /// chain; calling it on a binding and discarding the result silently
    /// keeps the non-grouped default.
    #[test]
    fn grouped_runner_rolls_back_the_whole_batch() {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        let ok = Migration::unapplied("V1__ok", "CREATE TABLE ok (id INTEGER PRIMARY KEY);")
            .expect("valid migration name");
        let failing =
            Migration::unapplied("V2__failing", "CREATE TABLE ok (id INTEGER PRIMARY KEY);")
                .expect("valid migration name");

        Runner::new(&[ok, failing])
            .set_grouped(true)
            .run(&mut conn)
            .expect_err("V2 duplicates V1's table");

        assert!(table_exists(&conn, "refinery_schema_history"));
        assert_eq!(
            history_row_count(&conn),
            0,
            "the batch rolled back its history rows"
        );
        assert!(
            !table_exists(&conn, "ok"),
            "the batch rolled back V1's table"
        );
    }

    // ---- Step 157: the database and the code agree ----

    /// A connection with every migration of this build applied.
    fn migrated() -> Connection {
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        run_migrations(&mut conn).expect("migrated");
        conn
    }

    /// The error a refused open gave, as `(database, reason)`.
    fn refusal(result: Result<(), KupferdrahtError>) -> (String, String) {
        match result {
            Err(KupferdrahtError::IncompatibleStore { database, reason }) => (database, reason),
            other => panic!("expected IncompatibleStore, got {other:?}"),
        }
    }

    /// A migration a newer build applied is unknown here: refused by name.
    #[test]
    fn a_database_newer_than_the_code_is_refused_by_name() {
        let mut conn = migrated();
        conn.execute(
            "INSERT INTO refinery_schema_history (version, name, applied_on, checksum) \
             VALUES (9999, 'from_a_newer_build', '2026-11-10T00:00:00Z', '0')",
            [],
        )
        .expect("a newer build's history row");

        let (database, reason) = refusal(run_migrations(&mut conn));

        assert_eq!(
            (database.as_str(), reason.as_str()),
            ("kupferdraht.db", "newer")
        );
    }

    /// An applied migration whose content differs from this build's: refused.
    #[test]
    fn a_database_whose_migration_differs_is_refused_by_name() {
        let mut conn = migrated();
        conn.execute(
            "UPDATE refinery_schema_history SET checksum = '1' \
             WHERE version = (SELECT MIN(version) FROM refinery_schema_history)",
            [],
        )
        .expect("a changed checksum");

        let (database, reason) = refusal(run_migrations(&mut conn));

        assert_eq!(
            (database.as_str(), reason.as_str()),
            ("kupferdraht.db", "divergent")
        );
    }

    /// The one exception: an older schema is migrated, and then matches —
    /// a second open finds nothing to do.
    #[test]
    fn an_older_database_is_migrated_and_then_matches() {
        let mut versions: Vec<i32> = embedded::migrations::runner()
            .get_migrations()
            .iter()
            .map(Migration::version)
            .collect();
        versions.sort_unstable();
        let before_last = versions[versions.len() - 2];
        let mut conn = Connection::open_in_memory().expect("in-memory db");
        embedded::migrations::runner()
            .set_grouped(true)
            .set_target(refinery::Target::Version(before_last))
            .run(&mut conn)
            .expect("an older build's schema");
        assert_eq!(
            history_row_count(&conn),
            i64::try_from(versions.len() - 1).expect("count")
        );

        run_migrations(&mut conn).expect("migrated forward");
        run_migrations(&mut conn).expect("equal: nothing to do");

        assert_eq!(
            history_row_count(&conn),
            i64::try_from(versions.len()).expect("count")
        );
    }
}
